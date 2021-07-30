use crate::packet::SerializedPacket;
use crate::Notification;
use core::convert::TryInto;
use core::ops::Add;
use embedded_time::duration::{Generic, Milliseconds};
use embedded_time::{Clock, Instant};
use heapless::{FnvIndexMap, FnvIndexSet, IndexMap, IndexSet};
use mqttrust::encoding::v4::*;

#[allow(unused)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum MqttConnectionStatus {
    Handshake,
    Connected,
    Disconnected,
}

#[derive(Debug, PartialEq)]
pub enum StateError {
    /// Broker's error reply to client's connect packet
    Connect(ConnectReturnCode),
    /// Invalid state for a given operation
    InvalidState,
    /// Received a packet (ack) which isn't asked for
    Unsolicited,
    /// Last pingreq isn't acked
    AwaitPingResp,
    /// Received a wrong packet while waiting for another packet
    WrongPacket,
    PayloadEncoding,
    InvalidUtf8,
    /// The maximum number of messages allowed to be simultaneously in-flight has been reached.
    MaxMessagesInflight,
    /// Non-zero QoS publications require PID
    PidMissing,
    InvalidHeader,
}

/// State of the mqtt connection.
///
/// Methods will just modify the state of the object without doing any network
/// operations This abstracts the functionality better so that it's easy to
/// switch between synchronous code, tokio (or) async/await
///
/// **Lifetimes**:
/// - 'a: The lifetime of the packet fields, backed by a slice buffer
///
/// **Generics**:
/// - O: The output timer used for keeping track of keep-alive ping-pongs. Must
///   implement the [`embedded_hal::timer::CountDown`] trait
pub struct MqttState<TIM> {
    /// Connection status
    pub connection_status: MqttConnectionStatus,
    /// Status of last ping
    pub await_pingresp: bool,
    /// Packet id of the last outgoing packet
    pub last_pid: Pid,
    /// Outgoing QoS 1, 2 publishes which aren't acked yet
    pub(crate) outgoing_pub: FnvIndexMap<u16, Inflight<TIM, 1024>, 2>,
    /// Packet ids of released QoS 2 publishes
    pub outgoing_rel: FnvIndexSet<u16, 1>,
    /// Packet ids on incoming QoS 2 publishes
    pub incoming_pub: FnvIndexSet<u16, 1>,
    last_ping: StartTime<TIM>,
}

impl<TIM> MqttState<TIM>
where
    TIM: Add<Milliseconds, Output = TIM> + PartialOrd + Copy,
{
    /// Creates new mqtt state. Same state should be used during a
    /// connection for persistent sessions while new state should
    /// instantiated for clean sessions
    pub fn new() -> Self {
        MqttState {
            connection_status: MqttConnectionStatus::Disconnected,
            await_pingresp: false,
            last_pid: Pid::new(),

            outgoing_pub: IndexMap::new(),
            outgoing_rel: IndexSet::new(),
            incoming_pub: IndexSet::new(),
            last_ping: StartTime::default(),
        }
    }

    /// Consolidates handling of all outgoing mqtt packet logic. Returns a
    /// packet which should be put on to the network by the eventloop
    pub fn handle_outgoing_packet<'b>(
        &mut self,
        packet: Packet<'b>,
    ) -> Result<Packet<'b>, StateError> {
        match packet {
            Packet::Pingreq => self.handle_outgoing_ping(),
            _ => unreachable!(),
        }
    }

    /// Consolidates handling of all outgoing mqtt packet logic. Returns a
    /// packet which should be put on to the network by the eventloop
    pub fn handle_outgoing_request(
        &mut self,
        request: &mut SerializedPacket<'_>,
        now: &TIM,
    ) -> Result<(), StateError> {
        match request.header()?.typ {
            PacketType::Publish => self.handle_outgoing_publish(request, now)?,
            PacketType::Subscribe => request.set_pid(self.next_pid())?,
            PacketType::Unsubscribe => request.set_pid(self.next_pid())?,
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Consolidates handling of all incoming mqtt packets. Returns a
    /// `Notification` which for the user to consume and `Packet` which for the
    /// eventloop to put on the network E.g For incoming QoS1 publish packet,
    /// this method returns (Publish, Puback). Publish packet will be forwarded
    /// to user and Pubck packet will be written to network
    pub fn handle_incoming_packet<'b>(
        &mut self,
        packet: Packet<'b>,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        match packet {
            Packet::Connack(connack) => self
                .handle_incoming_connack(connack)
                .map(|()| (Notification::ConnAck.into(), None)),
            Packet::Pingresp => self.handle_incoming_pingresp(),
            Packet::Publish(publish) => self.handle_incoming_publish(publish),
            Packet::Suback(suback) => self.handle_incoming_suback(suback),
            Packet::Unsuback(pid) => self.handle_incoming_unsuback(pid),
            Packet::Puback(pid) => self.handle_incoming_puback(pid),
            Packet::Pubrec(pid) => self.handle_incoming_pubrec(pid),
            Packet::Pubrel(pid) => self.handle_incoming_pubrel(pid),
            Packet::Pubcomp(pid) => self.handle_incoming_pubcomp(pid),
            _ => {
                defmt::error!("Invalid incoming packet!");
                Ok((None, None))
            }
        }
    }

    /// Adds next packet identifier to QoS 1 and 2 publish packets and returns
    /// it by wrapping publish in packet
    fn handle_outgoing_publish(
        &mut self,
        request: &mut SerializedPacket<'_>,
        now: &TIM,
    ) -> Result<(), StateError> {
        match request.header()?.qos {
            QoS::AtMostOnce => {}
            QoS::AtLeastOnce => {
                let pid = self.next_pid();
                self.outgoing_pub
                    .insert(pid.get(), Inflight::new(StartTime::new(*now), &request.0))
                    .map_err(|_| StateError::MaxMessagesInflight)?;
                request.set_pid(pid)?;
            }
            QoS::ExactlyOnce => {
                let pid = self.next_pid();
                self.outgoing_pub
                    .insert(pid.get(), Inflight::new(StartTime::new(*now), &request.0))
                    .map_err(|_| StateError::MaxMessagesInflight)?;
                request.set_pid(pid)?;
            }
        }
        Ok(())
    }

    /// Iterates through the list of stored publishes and removes the publish
    /// with the matching packet identifier. Removal is now a O(n) operation.
    /// This should be usually ok in case of acks due to ack ordering in normal
    /// conditions. But in cases where the broker doesn't guarantee the order of
    /// acks, the performance won't be optimal
    fn handle_incoming_puback(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        if self.outgoing_pub.contains_key(&pid.get()) {
            let _publish = self.outgoing_pub.remove(&pid.get());

            let request = None;
            let notification = Some(Notification::Puback(pid));
            Ok((notification, request))
        } else {
            defmt::error!("Unsolicited puback packet: {:?}", pid.get());
            Err(StateError::Unsolicited)
        }
    }

    fn handle_incoming_suback<'a>(
        &mut self,
        suback: Suback<'a>,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        // TODO:
        // let request = None;
        // let notification = Some(Notification::Suback(suback));
        Ok((None, None))
    }

    fn handle_incoming_unsuback(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        let request = None;
        let notification = Some(Notification::Unsuback(pid));
        Ok((notification, request))
    }

    /// Iterates through the list of stored publishes and removes the publish with the
    /// matching packet identifier. Removal is now a O(n) operation. This should be
    /// usually ok in case of acks due to ack ordering in normal conditions. But in cases
    /// where the broker doesn't guarantee the order of acks, the performance won't be optimal
    fn handle_incoming_pubrec(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        if self.outgoing_pub.contains_key(&pid.get()) {
            self.outgoing_pub.remove(&pid.get());
            self.outgoing_rel
                .insert(pid.get())
                .map_err(|_| StateError::InvalidState)?;

            let reply = Some(Packet::Pubrel(pid));
            let notification = Some(Notification::Pubrec(pid));
            Ok((notification, reply))
        } else {
            defmt::error!("Unsolicited pubrec packet: {:?}", pid.get());
            Err(StateError::Unsolicited)
        }
    }

    /// Results in a publish notification in all the QoS cases. Replys with an ack
    /// in case of QoS1 and Replys rec in case of QoS while also storing the message
    fn handle_incoming_publish<'b>(
        &mut self,
        publish: Publish<'b>,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        let qospid = (publish.qos, publish.pid);
        let notification = Notification::Publish(publish.try_into()?);

        let request = match qospid {
            (QoS::AtMostOnce, _) => None,
            (QoS::AtLeastOnce, Some(pid)) => Some(Packet::Puback(pid)),
            (QoS::ExactlyOnce, Some(pid)) => {
                self.incoming_pub.insert(pid.get()).map_err(|_| {
                    defmt::error!("Failed to insert incoming pub!");
                    StateError::InvalidState
                })?;

                Some(Packet::Pubrec(pid))
            }
            _ => return Err(StateError::InvalidHeader),
        };
        Ok((Some(notification), request))
    }

    fn handle_incoming_pubrel(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        if self.incoming_pub.contains(&pid.get()) {
            self.incoming_pub.remove(&pid.get());
            let reply = Packet::Pubcomp(pid);
            Ok((None, Some(reply)))
        } else {
            defmt::error!("Unsolicited pubrel packet: {:?}", pid.get());
            Err(StateError::Unsolicited)
        }
    }

    fn handle_incoming_pubcomp(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        if self.outgoing_rel.contains(&pid.get()) {
            self.outgoing_rel.remove(&pid.get());
            let notification = Some(Notification::Pubcomp(pid));
            let reply = None;
            Ok((notification, reply))
        } else {
            defmt::error!("Unsolicited pubcomp packet: {:?}", pid.get());
            Err(StateError::Unsolicited)
        }
    }

    /// check when the last control packet/pingreq packet is received and return
    /// the status which tells if keep alive time has exceeded
    /// NOTE: status will be checked for zero keepalive times also
    fn handle_outgoing_ping<'b>(&mut self) -> Result<Packet<'b>, StateError> {
        // raise error if last ping didn't receive ack
        if self.await_pingresp {
            defmt::error!("Error awaiting for last ping response");
            return Err(StateError::AwaitPingResp);
        }

        self.await_pingresp = true;

        defmt::trace!("Pingreq");

        Ok(Packet::Pingreq)
    }

    fn handle_incoming_pingresp(
        &mut self,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), StateError> {
        self.await_pingresp = false;
        defmt::trace!("Pingresp");
        Ok((None, None))
    }

    pub(crate) fn handle_outgoing_connect(&mut self) {
        self.connection_status = MqttConnectionStatus::Handshake;
    }

    pub fn handle_incoming_connack(&mut self, connack: Connack) -> Result<(), StateError> {
        match connack.code {
            ConnectReturnCode::Accepted
                if self.connection_status == MqttConnectionStatus::Handshake =>
            {
                defmt::debug!("MQTT connected!");
                self.connection_status = MqttConnectionStatus::Connected;
                Ok(())
            }
            ConnectReturnCode::Accepted
                if self.connection_status != MqttConnectionStatus::Handshake =>
            {
                defmt::error!(
                    "Invalid state. Expected = {:?}, Current = {:?}",
                    MqttConnectionStatus::Handshake,
                    self.connection_status
                );
                self.connection_status = MqttConnectionStatus::Disconnected;
                Err(StateError::InvalidState)
            }
            code => {
                defmt::error!("Connection failed. Connection error = {:?}", code as u8);
                self.connection_status = MqttConnectionStatus::Disconnected;
                Err(StateError::Connect(code))
            }
        }
    }

    fn next_pid(&mut self) -> Pid {
        self.last_pid = self.last_pid + 1;
        self.last_pid
    }

    pub(crate) fn last_ping_entry(&mut self) -> &mut StartTime<TIM> {
        &mut self.last_ping
    }

    pub(crate) fn retries(
        &mut self,
        now: TIM,
        interval: Milliseconds,
    ) -> impl Iterator<Item = (&u16, &mut Inflight<TIM, 1024>)> + '_ {
        self.outgoing_pub
            .iter_mut()
            .filter(move |(_, inflight)| inflight.last_touch.has_elapsed(&now, interval))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartTime<T>(Option<T>);

impl<T> Default for StartTime<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> StartTime<T> {
    pub fn new(start_time: T) -> Self {
        Self(start_time.into())
    }

    pub fn or_insert(&mut self, now: T) -> &mut Self {
        self.0.get_or_insert(now);
        self
    }

    pub fn insert(&mut self, now: T) {
        self.0.replace(now);
    }
}

impl<T> StartTime<T>
where
    T: Add<Milliseconds, Output = T> + PartialOrd + Copy,
{
    /// Check whether an interval has elapsed since this start time.
    pub fn has_elapsed(&self, now: &T, interval: Milliseconds) -> bool {
        if let Some(start_time) = self.0 {
            let elapse_time = start_time + interval;
            elapse_time <= *now
        } else {
            false
        }
    }
}

impl<O> defmt::Format for StartTime<Instant<O>>
where
    O: Clock,
    Generic<O::T>: TryInto<Milliseconds>,
{
    fn format(&self, fmt: defmt::Formatter) {
        let start_time = self
            .0
            .and_then(|t| t.duration_since_epoch().try_into().ok())
            .unwrap_or(Milliseconds(0u32))
            .0;
        defmt::write!(fmt, "{=u32}", start_time)
    }
}

/// Client publication message data.
#[derive(Debug)]
pub(crate) struct Inflight<TIM, const L: usize> {
    /// A publish of non-zero QoS.
    publish: heapless::Vec<u8, L>,
    /// A timestmap used for retry and expiry.
    last_touch: StartTime<TIM>,
    // next_message_type, only relevant for Qos2, i.e., PUBREC, PUBREL or PUBCOMP.
}

impl<TIM, const L: usize> Inflight<TIM, L>
where
    TIM: Add<Milliseconds, Output = TIM> + PartialOrd + Copy,
{
    pub(crate) fn new(last_touch: StartTime<TIM>, publish: &[u8]) -> Self {
        // assert!(
        //     !matches!(publish.qos, QoS::AtMostOnce),
        //     "Only non-zero QoSs are allowed."
        // );
        Self {
            publish: heapless::Vec::from_slice(publish).unwrap(),
            last_touch,
        }
    }

    pub(crate) fn last_touch_entry(&mut self) -> &mut StartTime<TIM> {
        &mut self.last_touch
    }
}

impl<TIM, const L: usize> Inflight<TIM, L> {
    pub(crate) fn packet<'b>(&'b mut self, pid: u16) -> Result<&'b [u8], StateError> {
        let pid = pid.try_into().map_err(|_| StateError::PayloadEncoding)?;
        let mut packet = SerializedPacket(self.publish.as_mut());
        packet.set_pid(pid)?;
        Ok(packet.to_inner())
    }
}

#[cfg(test)]
mod test {
    use super::{
        Clock, Milliseconds, MqttConnectionStatus, MqttState, Packet, StartTime, StateError,
    };
    use crate::{packet::SerializedPacket, Notification};
    use core::convert::TryFrom;
    use embedded_time::{duration::Extensions, Instant};
    use mqttrust::{
        encoding::v4::{decode_slice, encode_slice, Pid},
        Publish, QoS,
    };

    impl<O> From<u32> for StartTime<Instant<O>>
    where
        O: Clock<T = u32>,
    {
        fn from(now: u32) -> Self {
            Self::new(Instant::new(now))
        }
    }

    fn build_publish<'a>(qos: QoS, pid: Option<u16>) -> Publish<'a> {
        let topic = "hello/world";
        let payload = &[1, 2, 3];

        let pid = match qos {
            QoS::AtMostOnce => None,
            QoS::AtLeastOnce => pid.and_then(|p| Pid::try_from(p).ok()),
            QoS::ExactlyOnce => pid.and_then(|p| Pid::try_from(p).ok()),
        };

        Publish {
            qos,
            pid,
            payload,
            dup: false,
            retain: false,
            topic_name: topic,
        }
    }

    fn build_mqttstate() -> MqttState<Milliseconds> {
        MqttState::new()
    }

    #[test]
    fn handle_outgoing_requests() {
        let buf = &mut [0u8; 256];
        let now = 0u32.milliseconds();
        let mut mqtt = build_mqttstate();

        // Publish
        let publish = Packet::Publish(build_publish(QoS::AtMostOnce, None));

        let len = encode_slice(&publish, buf).unwrap();

        // Packet id shouldn't be set and publish shouldn't be saved in queue
        mqtt.handle_outgoing_request(&mut SerializedPacket(&mut buf[..len]), &now)
            .unwrap();
        // assert_eq!(publish_out.qos, QoS::AtMostOnce);
        // assert_eq!(mqtt.outgoing_pub.len(), 0);

        // // Subscribe
        // let subscribe = SubscribeRequest {
        //     topics: Vec::from_slice(&[
        //         SubscribeTopic {
        //             topic_path: String::from("some/topic"),
        //             qos: QoS::AtLeastOnce,
        //         },
        //         SubscribeTopic {
        //             topic_path: String::from("some/other/topic"),
        //             qos: QoS::ExactlyOnce,
        //         },
        //     ])
        //     .unwrap(),
        // };

        // // Packet id should be set and subscribe shouldn't be saved in publish queue
        // mqtt.handle_outgoing_request(subscribe.try_into().unwrap(), buf, &now)
        //     .unwrap();
        // let mut topics_iter = subscribe_out.topics.iter();

        // assert_eq!(subscribe_out.pid, Pid::try_from(2).unwrap());
        // assert_eq!(
        //     topics_iter.next(),
        //     Some(&SubscribeTopic {
        //         qos: QoS::AtLeastOnce,
        //         topic_path: String::from("some/topic")
        //     })
        // );
        // assert_eq!(
        //     topics_iter.next(),
        //     Some(&SubscribeTopic {
        //         qos: QoS::ExactlyOnce,
        //         topic_path: String::from("some/other/topic")
        //     })
        // );
        // assert_eq!(topics_iter.next(), None);
        // assert_eq!(mqtt.outgoing_pub.len(), 0);

        // // Unsubscribe
        // let unsubscribe = UnsubscribeRequest {
        //     topics: Vec::from_slice(&[
        //         String::from("some/topic"),
        //         String::from("some/other/topic"),
        //     ])
        //     .unwrap(),
        // };

        // // Packet id should be set and subscribe shouldn't be saved in publish queue
        // let unsubscribe_out =
        //     match mqtt.handle_outgoing_request(unsubscribe.try_into().unwrap(), buf, &now) {
        //         Ok(Packet::Unsubscribe(p)) => p,
        //         _ => panic!("Invalid packet. Should've been a unsubscribe packet"),
        //     };
        // let mut topics_iter = unsubscribe_out.topics.iter();

        // assert_eq!(unsubscribe_out.pid, Pid::try_from(3).unwrap());
        // assert_eq!(topics_iter.next(), Some(&String::from("some/topic")));
        // assert_eq!(topics_iter.next(), Some(&String::from("some/other/topic")));
        // assert_eq!(topics_iter.next(), None);
        // assert_eq!(mqtt.outgoing_pub.len(), 0);
    }

    #[test]
    fn outgoing_publish_handle_should_set_pid_correctly_and_add_publish_to_queue_correctly() {
        let buf = &mut [0u8; 256];
        let now = 0u32.milliseconds();

        let mut mqtt = build_mqttstate();

        // QoS0 Publish
        let publish = Packet::Publish(build_publish(QoS::AtMostOnce, None));
        let len = encode_slice(&publish, buf).unwrap();
        let mut pkg = SerializedPacket(&mut buf[..len]);

        // Packet id shouldn't be set and publish shouldn't be saved in queue
        mqtt.handle_outgoing_publish(&mut pkg, &now).unwrap();

        let publish_out = match decode_slice(pkg.to_inner()).unwrap() {
            Some(Packet::Publish(p)) => p,
            _ => panic!(),
        };
        assert_eq!(publish_out.qos, QoS::AtMostOnce);
        assert_eq!(mqtt.outgoing_pub.len(), 0);

        // QoS1 Publish
        let publish = Packet::Publish(build_publish(QoS::AtLeastOnce, None));
        let len = encode_slice(&publish, buf).unwrap();
        let mut pkg = SerializedPacket(&mut buf[..len]);

        // Packet id should be set and publish should be saved in queue
        mqtt.handle_outgoing_publish(&mut pkg, &now).unwrap();
        let publish_out = match decode_slice(pkg.to_inner()).unwrap() {
            Some(Packet::Publish(p)) => p,
            _ => panic!(),
        };
        assert_eq!(publish_out.qos, QoS::AtLeastOnce);
        assert_eq!(publish_out.pid, Some(Pid::try_from(2).unwrap()));
        assert_eq!(mqtt.outgoing_pub.len(), 1);

        let publish = Packet::Publish(build_publish(QoS::AtLeastOnce, None));
        let len = encode_slice(&publish, buf).unwrap();
        let mut pkg = SerializedPacket(&mut buf[..len]);

        // Packet id should be incremented and publish should be saved in queue
        mqtt.handle_outgoing_publish(&mut pkg, &now).unwrap();
        let publish_out = match decode_slice(pkg.to_inner()).unwrap() {
            Some(Packet::Publish(p)) => p,
            _ => panic!(),
        };
        assert_eq!(publish_out.qos, QoS::AtLeastOnce);
        assert_eq!(publish_out.pid, Some(Pid::try_from(3).unwrap()));
        assert_eq!(mqtt.outgoing_pub.len(), 2);

        // QoS1 Publish
        let publish = Packet::Publish(build_publish(QoS::ExactlyOnce, None));
        let len = encode_slice(&publish, buf).unwrap();
        let mut pkg = SerializedPacket(&mut buf[..len]);

        // Packet id should be set and publish should be saved in queue
        mqtt.handle_outgoing_publish(&mut pkg, &now).unwrap();
        let publish_out = match decode_slice(pkg.to_inner()).unwrap() {
            Some(Packet::Publish(p)) => p,
            _ => panic!(),
        };

        assert_eq!(publish_out.qos, QoS::ExactlyOnce);
        assert_eq!(publish_out.pid, Some(Pid::try_from(4).unwrap()));
        assert_eq!(mqtt.outgoing_pub.len(), 3);

        let publish = Packet::Publish(build_publish(QoS::ExactlyOnce, None));
        let len = encode_slice(&publish, buf).unwrap();
        let mut pkg = SerializedPacket(&mut buf[..len]);

        // Packet id should be incremented and publish should be saved in queue
        mqtt.handle_outgoing_publish(&mut pkg, &now).unwrap();
        let publish_out = match decode_slice(pkg.to_inner()).unwrap() {
            Some(Packet::Publish(p)) => p,
            _ => panic!(),
        };

        assert_eq!(publish_out.qos, QoS::ExactlyOnce);
        assert_eq!(publish_out.pid, Some(Pid::try_from(5).unwrap()));
        assert_eq!(mqtt.outgoing_pub.len(), 4);
    }

    #[test]
    fn incoming_publish_should_be_added_to_queue_correctly() {
        let mut mqtt = build_mqttstate();

        // QoS0, 1, 2 Publishes
        let publish1 = build_publish(QoS::AtMostOnce, Some(1));
        let publish2 = build_publish(QoS::AtLeastOnce, Some(2));
        let publish3 = build_publish(QoS::ExactlyOnce, Some(3));

        mqtt.handle_incoming_publish(publish1).unwrap();
        mqtt.handle_incoming_publish(publish2).unwrap();
        mqtt.handle_incoming_publish(publish3).unwrap();

        // only qos2 publish should be add to queue
        assert_eq!(mqtt.incoming_pub.len(), 1);
        assert!(mqtt.incoming_pub.contains(&3));
    }

    #[test]
    fn incoming_qos2_publish_should_send_rec_to_network_and_publish_to_user() {
        let mut mqtt = build_mqttstate();
        let publish = build_publish(QoS::ExactlyOnce, Some(1));

        let (notification, request) = mqtt.handle_incoming_publish(publish).unwrap();

        match notification {
            Some(Notification::Publish(publish)) => assert_eq!(publish.qospid, QoS::ExactlyOnce),
            _ => panic!("Invalid notification: {:?}", notification),
        }

        match request {
            Some(Packet::Pubrec(pid)) => assert_eq!(pid.get(), 1),
            _ => panic!("Invalid network request: {:?}", request),
        }
    }

    #[test]
    fn incoming_puback_should_remove_correct_publish_from_queue() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];
        let now = 0u32.milliseconds();

        let publish1 = Packet::Publish(build_publish(QoS::AtLeastOnce, None));
        let len = encode_slice(&publish1, buf).unwrap();
        let mut pkg1 = SerializedPacket(&mut buf[..len]);
        mqtt.handle_outgoing_publish(&mut pkg1, &now).unwrap();

        let publish2 = Packet::Publish(build_publish(QoS::ExactlyOnce, None));
        let len = encode_slice(&publish2, buf).unwrap();
        let mut pkg2 = SerializedPacket(&mut buf[..len]);
        mqtt.handle_outgoing_publish(&mut pkg2, &now).unwrap();

        mqtt.handle_incoming_puback(Pid::try_from(2).unwrap())
            .unwrap();
        assert_eq!(mqtt.outgoing_pub.len(), 1);

        let backup = mqtt.outgoing_pub.get(&3);
        // assert_eq!(backup.unwrap().publish.qos, QoS::ExactlyOnce);

        mqtt.handle_incoming_puback(Pid::try_from(3).unwrap())
            .unwrap();
        assert_eq!(mqtt.outgoing_pub.len(), 0);
    }

    #[test]
    fn incoming_pubrec_should_release_correct_publish_from_queue_and_add_releaseid_to_rel_queue() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];
        let now = 0u32.milliseconds();

        let publish1 = Packet::Publish(build_publish(QoS::AtLeastOnce, None));
        let len = encode_slice(&publish1, buf).unwrap();
        let mut pkg1 = SerializedPacket(&mut buf[..len]);
        mqtt.handle_outgoing_publish(&mut pkg1, &now).unwrap();

        let publish2 = Packet::Publish(build_publish(QoS::ExactlyOnce, None));
        let len = encode_slice(&publish2, buf).unwrap();
        let mut pkg2 = SerializedPacket(&mut buf[..len]);
        mqtt.handle_outgoing_publish(&mut pkg2, &now).unwrap();

        mqtt.handle_incoming_pubrec(Pid::try_from(3).unwrap())
            .unwrap();
        assert_eq!(mqtt.outgoing_pub.len(), 1);

        // check if the remaining element's pid is 2
        let backup = mqtt.outgoing_pub.get(&2);
        // assert_eq!(backup.unwrap().publish.qos, QoS::AtLeastOnce);

        assert_eq!(mqtt.outgoing_rel.len(), 1);

        // check if the  element's pid is 3
        assert!(mqtt.outgoing_rel.contains(&3));
    }

    #[test]
    fn incoming_pubrec_should_send_release_to_network_and_nothing_to_user() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];
        let now = 0u32.milliseconds();
        let pid = Pid::try_from(2).unwrap();
        assert_eq!(pid.get(), 2);

        let publish = Packet::Publish(build_publish(QoS::ExactlyOnce, None));
        let len = encode_slice(&publish, buf).unwrap();
        let mut pkg = SerializedPacket(&mut buf[..len]);
        mqtt.handle_outgoing_publish(&mut pkg, &now).unwrap();

        let (notification, request) = mqtt.handle_incoming_pubrec(pid).unwrap();

        assert_eq!(notification, Some(Notification::Pubrec(pid)));
        assert_eq!(request, Some(Packet::Pubrel(pid)));
    }

    #[test]
    fn incoming_pubrel_should_send_comp_to_network_and_nothing_to_user() {
        let mut mqtt = build_mqttstate();
        let publish = build_publish(QoS::ExactlyOnce, Some(1));

        let pid = Pid::try_from(1).unwrap();
        assert_eq!(pid.get(), 1);

        mqtt.handle_incoming_publish(publish).unwrap();

        let (notification, request) = mqtt.handle_incoming_pubrel(pid).unwrap();
        assert_eq!(notification, None);
        assert_eq!(request, Some(Packet::Pubcomp(pid)));
    }

    #[test]
    fn incoming_pubcomp_should_release_correct_pid_from_release_queue() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];
        let now = 0u32.milliseconds();
        let publish = Packet::Publish(build_publish(QoS::ExactlyOnce, None));
        let len = encode_slice(&publish, buf).unwrap();
        let mut pkg = SerializedPacket(&mut buf[..len]);

        let pid = Pid::try_from(2).unwrap();

        mqtt.handle_outgoing_publish(&mut pkg, &now).unwrap();
        mqtt.handle_incoming_pubrec(pid).unwrap();

        mqtt.handle_incoming_pubcomp(pid).unwrap();
        assert_eq!(mqtt.outgoing_pub.len(), 0);
    }

    #[test]
    fn outgoing_ping_handle_should_throw_errors_for_no_pingresp() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];
        let now = 0u32.milliseconds();
        mqtt.connection_status = MqttConnectionStatus::Connected;
        assert_eq!(mqtt.handle_outgoing_ping(), Ok(Packet::Pingreq));
        assert!(mqtt.await_pingresp);

        // network activity other than pingresp
        let publish = Packet::Publish(build_publish(QoS::AtLeastOnce, None));
        let len = encode_slice(&publish, buf).unwrap();
        let mut pkg = SerializedPacket(&mut buf[..len]);

        mqtt.handle_outgoing_publish(&mut pkg, &now).unwrap();
        mqtt.handle_incoming_packet(Packet::Puback(Pid::try_from(2).unwrap()))
            .unwrap();

        // should throw error because we didn't get pingresp for previous ping
        assert_eq!(mqtt.handle_outgoing_ping(), Err(StateError::AwaitPingResp));
    }

    #[test]
    fn outgoing_ping_handle_should_succeed_if_pingresp_is_received() {
        let mut mqtt = build_mqttstate();

        mqtt.connection_status = MqttConnectionStatus::Connected;

        // should ping
        assert_eq!(mqtt.handle_outgoing_ping(), Ok(Packet::Pingreq));
        assert!(mqtt.await_pingresp);
        assert_eq!(
            mqtt.handle_incoming_packet(Packet::Pingresp),
            Ok((None, None))
        );
        assert!(!mqtt.await_pingresp);

        // should ping
        assert_eq!(mqtt.handle_outgoing_ping(), Ok(Packet::Pingreq));
        assert!(mqtt.await_pingresp);
    }
}
