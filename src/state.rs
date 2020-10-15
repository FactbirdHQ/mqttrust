use crate::requests::UnsubscribeRequest;
use crate::{Notification, PublishPayload, PublishRequest, Request, Subscribe, SubscribeRequest};
use core::convert::TryInto;
use heapless::{consts, FnvIndexMap, FnvIndexSet, IndexMap, IndexSet};
use mqttrs::*;

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
pub struct MqttState<O, P> {
    /// Connection status
    pub connection_status: MqttConnectionStatus,
    /// Status of last ping
    pub await_pingresp: bool,
    /// Last outgoing packet time
    pub last_outgoing_timer: O,
    /// Packet id of the last outgoing packet
    pub last_pid: Pid,
    /// Outgoing QoS 1, 2 publishes which aren't acked yet
    pub outgoing_pub: FnvIndexMap<u16, PublishRequest<P>, consts::U4>,
    /// Packet ids of released QoS 2 publishes
    pub outgoing_rel: FnvIndexSet<u16, consts::U4>,
    /// Packet ids on incoming QoS 2 publishes
    pub incoming_pub: FnvIndexSet<u16, consts::U2>,
}

impl<O, P> MqttState<O, P>
where
    O: embedded_hal::timer::CountDown,
    O::Time: From<u32>,
    P: PublishPayload + Clone,
{
    /// Creates new mqtt state. Same state should be used during a
    /// connection for persistent sessions while new state should
    /// instantiated for clean sessions
    pub fn new(outgoing_timer: O) -> Self {
        MqttState {
            connection_status: MqttConnectionStatus::Disconnected,
            await_pingresp: false,
            last_outgoing_timer: outgoing_timer,
            last_pid: Pid::new(),

            outgoing_pub: IndexMap::new(),
            outgoing_rel: IndexSet::new(),
            incoming_pub: IndexSet::new(),
        }
    }

    /// Consolidates handling of all outgoing mqtt packet logic. Returns a
    /// packet which should be put on to the network by the eventloop
    pub fn handle_outgoing_packet<'a>(
        &mut self,
        packet: Packet<'a>,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        let out = match packet {
            Packet::Pingreq => self.handle_outgoing_ping()?,
            _ => unimplemented!(),
        };

        Ok((None, Some(out)))
    }

    /// Consolidates handling of all outgoing mqtt packet logic. Returns a
    /// packet which should be put on to the network by the eventloop
    pub fn handle_outgoing_request<'a>(
        &mut self,
        request: Request<P>,
        buf: &'a mut [u8],
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        let out = match request {
            Request::Publish(publish) => self.handle_outgoing_publish(publish, buf)?,
            Request::Subscribe(subscribe) => self.handle_outgoing_subscribe(subscribe)?,
            Request::Unsubscribe(unsubscribe) => self.handle_outgoing_unsubscribe(unsubscribe)?,
            _ => unimplemented!(),
        };

        Ok((None, Some(out)))
    }

    /// Consolidates handling of all incoming mqtt packets. Returns a
    /// `Notification` which for the user to consume and `Packet` which for the
    /// eventloop to put on the network E.g For incoming QoS1 publish packet,
    /// this method returns (Publish, Puback). Publish packet will be forwarded
    /// to user and Pubck packet will be written to network
    pub fn handle_incoming_packet<'a>(
        &mut self,
        packet: Packet<'a>,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        match packet {
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
    fn handle_outgoing_publish<'a>(
        &mut self,
        request: PublishRequest<P>,
        buf: &'a mut [u8],
    ) -> Result<Packet<'a>, StateError> {
        let qospid = match request.qos {
            QoS::AtMostOnce => QosPid::AtMostOnce,
            QoS::AtLeastOnce => {
                let pid = self.next_pid();
                self.outgoing_pub
                    .insert(pid.get(), request.clone())
                    .map_err(|_| StateError::InvalidState)?;
                QosPid::AtLeastOnce(pid)
            }
            QoS::ExactlyOnce => {
                let pid = self.next_pid();
                self.outgoing_pub
                    .insert(pid.get(), request.clone())
                    .map_err(|_| StateError::InvalidState)?;
                QosPid::ExactlyOnce(pid)
            }
        };

        let topic_len = request.topic_name.len();
        buf[..topic_len].copy_from_slice(request.topic_name.as_str().as_bytes());

        let len = request
            .payload
            .as_bytes(&mut buf[topic_len..])
            .map_err(|_| StateError::PayloadEncoding)?;

        let publish = Publish {
            dup: request.dup,
            qospid,
            retain: request.retain,
            topic_name: core::str::from_utf8(&buf[..topic_len])
                .map_err(|_| StateError::PayloadEncoding)?,
            payload: &buf[topic_len..topic_len + len],
        };

        defmt::trace!(
            "Publish. Topic = {:str}, Payload Size = {:?}",
            publish.topic_name,
            publish.payload.len()
        );

        Ok(publish.into())
    }

    /// Iterates through the list of stored publishes and removes the publish
    /// with the matching packet identifier. Removal is now a O(n) operation.
    /// This should be usually ok in case of acks due to ack ordering in normal
    /// conditions. But in cases where the broker doesn't guarantee the order of
    /// acks, the performance won't be optimal
    fn handle_incoming_puback<'a>(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
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
        suback: Suback,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        let request = None;
        let notification = Some(Notification::Suback(suback));
        Ok((notification, request))
    }

    fn handle_incoming_unsuback<'a>(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        let request = None;
        let notification = Some(Notification::Unsuback(pid));
        Ok((notification, request))
    }

    /// Iterates through the list of stored publishes and removes the publish with the
    /// matching packet identifier. Removal is now a O(n) operation. This should be
    /// usually ok in case of acks due to ack ordering in normal conditions. But in cases
    /// where the broker doesn't guarantee the order of acks, the performance won't be optimal
    fn handle_incoming_pubrec<'a>(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        if self.outgoing_pub.contains_key(&pid.get()) {
            let _publish = self.outgoing_pub.remove(&pid.get());
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
    fn handle_incoming_publish<'a>(
        &mut self,
        publish: Publish<'a>,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        let qospid = publish.qospid;

        match qospid {
            QosPid::AtMostOnce => {
                let notification = Notification::Publish(publish.try_into()?);
                Ok((Some(notification), None))
            }
            QosPid::AtLeastOnce(pid) => {
                let request = Packet::Puback(pid);
                let notification = Notification::Publish(publish.try_into()?);
                Ok((Some(notification), Some(request)))
            }
            QosPid::ExactlyOnce(pid) => {
                let reply = Packet::Pubrec(pid);
                let notification = Notification::Publish(publish.try_into()?);

                self.incoming_pub
                    .insert(pid.get())
                    .map_err(|_| StateError::InvalidState)?;
                Ok((Some(notification), Some(reply)))
            }
        }
    }

    fn handle_incoming_pubrel<'a>(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        if self.incoming_pub.contains(&pid.get()) {
            self.incoming_pub.remove(&pid.get());
            let reply = Packet::Pubcomp(pid);
            Ok((None, Some(reply)))
        } else {
            defmt::error!("Unsolicited pubrel packet: {:?}", pid.get());
            Err(StateError::Unsolicited)
        }
    }

    fn handle_incoming_pubcomp<'a>(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
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
    fn handle_outgoing_ping<'a>(&mut self) -> Result<Packet<'a>, StateError> {
        // raise error if last ping didn't receive ack
        if self.await_pingresp {
            defmt::error!("Error awaiting for last ping response");
            return Err(StateError::AwaitPingResp);
        }

        self.await_pingresp = true;

        defmt::trace!("Pingreq");

        Ok(Packet::Pingreq)
    }

    fn handle_incoming_pingresp<'a>(
        &mut self,
    ) -> Result<(Option<Notification>, Option<Packet<'a>>), StateError> {
        self.await_pingresp = false;
        defmt::trace!("Pingresp");
        Ok((None, None))
    }

    fn handle_outgoing_subscribe<'a>(
        &mut self,
        subscribe_request: SubscribeRequest,
    ) -> Result<Packet<'a>, StateError> {
        defmt::trace!("Subscribe. Topics = ");
        for topic in &subscribe_request.topics {
            defmt::trace!("{:str}", topic.topic_path.as_str());
        }
        let subscription = Subscribe::new(self.next_pid(), subscribe_request.topics);

        Ok(subscription.into())
    }

    fn handle_outgoing_unsubscribe<'a>(
        &mut self,
        unsubscribe_request: UnsubscribeRequest,
    ) -> Result<Packet<'a>, StateError> {
        defmt::trace!("Unsubscribe. Topics = ");
        for topic in &unsubscribe_request.topics {
            defmt::trace!("{:str}", topic.as_str());
        }
        let unsubscription = Unsubscribe::new(self.next_pid(), unsubscribe_request.topics);
        Ok(unsubscription.into())
    }

    pub fn handle_outgoing_connect(&mut self) -> Result<(), StateError> {
        self.connection_status = MqttConnectionStatus::Handshake;
        Ok(())
    }

    pub fn handle_incoming_connack<'a>(&mut self, packet: Packet<'a>) -> Result<(), StateError> {
        let connack = match packet {
            Packet::Connack(connack) => connack,
            _packet => {
                defmt::error!("Invalid packet. Expecting connack!",);

                self.connection_status = MqttConnectionStatus::Disconnected;
                return Err(StateError::WrongPacket);
            }
        };

        match connack.code {
            ConnectReturnCode::Accepted
                if self.connection_status == MqttConnectionStatus::Handshake =>
            {
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
}

#[cfg(test)]
mod test {
    use super::{MqttConnectionStatus, MqttState, Packet, StateError};
    use crate::{Notification, PublishRequest, SubscribeRequest, UnsubscribeRequest};
    use core::convert::TryFrom;
    use embedded_hal::timer::CountDown;
    use heapless::{consts, String, Vec};
    use mqttrs::*;

    #[derive(Debug)]
    struct CdMock {
        time: u32,
    }

    impl CountDown for CdMock {
        type Error = core::convert::Infallible;
        type Time = u32;
        fn try_start<T>(&mut self, count: T) -> Result<(), Self::Error>
        where
            T: Into<Self::Time>,
        {
            self.time = count.into();
            Ok(())
        }
        fn try_wait(&mut self) -> nb::Result<(), Self::Error> {
            Ok(())
        }
    }

    fn build_outgoing_publish<'a>(qos: QoS) -> PublishRequest<Vec<u8, consts::U3>> {
        let topic = heapless::String::from("hello/world");
        let payload = Vec::from_slice(&[1, 2, 3]).unwrap();

        PublishRequest::new(topic, payload).qos(qos)
    }

    fn build_incoming_publish<'a>(qos: QoS, pid: u16) -> Publish<'a> {
        let topic = "hello/world";
        let payload = &[1, 2, 3];

        let qospid = match qos {
            QoS::AtMostOnce => QosPid::AtMostOnce,
            QoS::AtLeastOnce => QosPid::AtLeastOnce(Pid::try_from(pid).unwrap()),
            QoS::ExactlyOnce => QosPid::ExactlyOnce(Pid::try_from(pid).unwrap()),
        };

        Publish {
            qospid,
            payload,
            dup: false,
            retain: false,
            topic_name: topic,
        }
    }

    fn build_mqttstate<'a>() -> MqttState<CdMock, Vec<u8, consts::U3>> {
        let outgoing_timer = CdMock { time: 0 };
        MqttState::new(outgoing_timer)
    }

    #[test]
    fn handle_outgoing_requests() {
        let buf = &mut [0u8; 256];

        let mut mqtt = build_mqttstate();

        // Publish
        let publish = build_outgoing_publish(QoS::AtMostOnce);

        // Packet id shouldn't be set and publish shouldn't be saved in queue
        let publish_out = match mqtt.handle_outgoing_request(publish.into(), buf) {
            Ok((None, Some(Packet::Publish(p)))) => p,
            _ => panic!("Invalid packet. Should've been a publish packet"),
        };
        assert_eq!(publish_out.qospid, QosPid::AtMostOnce);
        assert_eq!(mqtt.outgoing_pub.len(), 0);

        // Subscribe
        let subscribe = SubscribeRequest {
            topics: Vec::from_slice(&[
                SubscribeTopic {
                    topic_path: String::from("some/topic"),
                    qos: QoS::AtLeastOnce,
                },
                SubscribeTopic {
                    topic_path: String::from("some/other/topic"),
                    qos: QoS::ExactlyOnce,
                },
            ])
            .unwrap(),
        };

        // Packet id should be set and subscribe shouldn't be saved in publish queue
        let subscribe_out = match mqtt.handle_outgoing_request(subscribe.into(), buf) {
            Ok((None, Some(Packet::Subscribe(p)))) => p,
            _ => panic!("Invalid packet. Should've been a subscribe packet"),
        };
        let mut topics_iter = subscribe_out.topics.iter();

        assert_eq!(subscribe_out.pid, Pid::try_from(2).unwrap());
        assert_eq!(
            topics_iter.next(),
            Some(&SubscribeTopic {
                qos: QoS::AtLeastOnce,
                topic_path: String::from("some/topic")
            })
        );
        assert_eq!(
            topics_iter.next(),
            Some(&SubscribeTopic {
                qos: QoS::ExactlyOnce,
                topic_path: String::from("some/other/topic")
            })
        );
        assert_eq!(topics_iter.next(), None);
        assert_eq!(mqtt.outgoing_pub.len(), 0);

        // Unsubscribe
        let unsubscribe = UnsubscribeRequest {
            topics: Vec::from_slice(&[
                String::from("some/topic"),
                String::from("some/other/topic"),
            ])
            .unwrap(),
        };

        // Packet id should be set and subscribe shouldn't be saved in publish queue
        let unsubscribe_out = match mqtt.handle_outgoing_request(unsubscribe.into(), buf) {
            Ok((None, Some(Packet::Unsubscribe(p)))) => p,
            _ => panic!("Invalid packet. Should've been a unsubscribe packet"),
        };
        let mut topics_iter = unsubscribe_out.topics.iter();

        assert_eq!(unsubscribe_out.pid, Pid::try_from(3).unwrap());
        assert_eq!(topics_iter.next(), Some(&String::from("some/topic")));
        assert_eq!(topics_iter.next(), Some(&String::from("some/other/topic")));
        assert_eq!(topics_iter.next(), None);
        assert_eq!(mqtt.outgoing_pub.len(), 0);
    }

    #[test]
    fn outgoing_publish_handle_should_set_pid_correctly_and_add_publish_to_queue_correctly() {
        let buf = &mut [0u8; 256];

        let mut mqtt = build_mqttstate();

        // QoS0 Publish
        let publish = build_outgoing_publish(QoS::AtMostOnce);

        // Packet id shouldn't be set and publish shouldn't be saved in queue
        let publish_out = match mqtt.handle_outgoing_publish(publish, buf) {
            Ok(Packet::Publish(p)) => p,
            _ => panic!("Invalid packet. Should've been a publish packet"),
        };
        assert_eq!(publish_out.qospid, QosPid::AtMostOnce);
        assert_eq!(mqtt.outgoing_pub.len(), 0);

        // QoS1 Publish
        let publish = build_outgoing_publish(QoS::AtLeastOnce);

        // Packet id should be set and publish should be saved in queue
        let publish_out = match mqtt.handle_outgoing_publish(publish.clone(), buf) {
            Ok(Packet::Publish(p)) => p,
            _ => panic!("Invalid packet. Should've been a publish packet"),
        };
        assert_eq!(
            publish_out.qospid,
            QosPid::AtLeastOnce(Pid::try_from(2).unwrap())
        );
        assert_eq!(mqtt.outgoing_pub.len(), 1);

        // Packet id should be incremented and publish should be saved in queue
        let publish_out = match mqtt.handle_outgoing_publish(publish.clone(), buf) {
            Ok(Packet::Publish(p)) => p,
            _ => panic!("Invalid packet. Should've been a publish packet"),
        };
        assert_eq!(
            publish_out.qospid,
            QosPid::AtLeastOnce(Pid::try_from(3).unwrap())
        );
        assert_eq!(mqtt.outgoing_pub.len(), 2);

        // QoS1 Publish
        let publish = build_outgoing_publish(QoS::ExactlyOnce);

        // Packet id should be set and publish should be saved in queue
        let publish_out = match mqtt.handle_outgoing_publish(publish.clone(), buf) {
            Ok(Packet::Publish(p)) => p,
            _ => panic!("Invalid packet. Should've been a publish packet"),
        };
        assert_eq!(
            publish_out.qospid,
            QosPid::ExactlyOnce(Pid::try_from(4).unwrap())
        );
        assert_eq!(mqtt.outgoing_pub.len(), 3);

        // Packet id should be incremented and publish should be saved in queue
        let publish_out = match mqtt.handle_outgoing_publish(publish.clone(), buf) {
            Ok(Packet::Publish(p)) => p,
            _ => panic!("Invalid packet. Should've been a publish packet"),
        };
        assert_eq!(
            publish_out.qospid,
            QosPid::ExactlyOnce(Pid::try_from(5).unwrap())
        );
        assert_eq!(mqtt.outgoing_pub.len(), 4);
    }

    #[test]
    fn incoming_publish_should_be_added_to_queue_correctly() {
        let mut mqtt = build_mqttstate();

        // QoS0, 1, 2 Publishes
        let publish1 = build_incoming_publish(QoS::AtMostOnce, 1);
        let publish2 = build_incoming_publish(QoS::AtLeastOnce, 2);
        let publish3 = build_incoming_publish(QoS::ExactlyOnce, 3);

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
        let publish = build_incoming_publish(QoS::ExactlyOnce, 1);

        let (notification, request) = mqtt.handle_incoming_publish(publish).unwrap();

        match notification {
            Some(Notification::Publish(publish)) => assert_eq!(
                publish.qospid,
                QosPid::ExactlyOnce(Pid::try_from(1).unwrap())
            ),
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

        let publish1 = build_outgoing_publish(QoS::AtLeastOnce);
        let publish2 = build_outgoing_publish(QoS::ExactlyOnce);

        mqtt.handle_outgoing_publish(publish1, buf).unwrap();
        mqtt.handle_outgoing_publish(publish2, buf).unwrap();

        mqtt.handle_incoming_puback(Pid::try_from(2).unwrap())
            .unwrap();
        assert_eq!(mqtt.outgoing_pub.len(), 1);

        let backup = mqtt.outgoing_pub.get(&3);
        assert_eq!(backup.unwrap().qos, QoS::ExactlyOnce);

        mqtt.handle_incoming_puback(Pid::try_from(3).unwrap())
            .unwrap();
        assert_eq!(mqtt.outgoing_pub.len(), 0);
    }

    #[test]
    fn incoming_pubrec_should_release_correct_publish_from_queue_and_add_releaseid_to_rel_queue() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];

        let publish1 = build_outgoing_publish(QoS::AtLeastOnce);
        let publish2 = build_outgoing_publish(QoS::ExactlyOnce);

        mqtt.handle_outgoing_publish(publish1, buf).unwrap();
        mqtt.handle_outgoing_publish(publish2, buf).unwrap();

        mqtt.handle_incoming_pubrec(Pid::try_from(3).unwrap())
            .unwrap();
        assert_eq!(mqtt.outgoing_pub.len(), 1);

        // check if the remaining element's pid is 2
        let backup = mqtt.outgoing_pub.get(&2);
        assert_eq!(backup.unwrap().qos, QoS::AtLeastOnce);

        assert_eq!(mqtt.outgoing_rel.len(), 1);

        // check if the  element's pid is 3
        assert!(mqtt.outgoing_rel.contains(&3));
    }

    #[test]
    fn incoming_pubrec_should_send_release_to_network_and_nothing_to_user() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];

        let publish = build_outgoing_publish(QoS::ExactlyOnce);
        mqtt.handle_outgoing_publish(publish, buf).unwrap();

        let (notification, request) = mqtt
            .handle_incoming_pubrec(Pid::try_from(2).unwrap())
            .unwrap();

        match notification {
            Some(Notification::Pubrec(pid)) => assert_eq!(pid.get(), 2),
            _ => panic!("Invalid notification"),
        }

        match request {
            Some(Packet::Pubrel(pid)) => assert_eq!(pid.get(), 2),
            _ => panic!("Invalid network request: {:?}", request),
        }
    }

    #[test]
    fn incoming_pubrel_should_send_comp_to_network_and_nothing_to_user() {
        let mut mqtt = build_mqttstate();
        let publish = build_incoming_publish(QoS::ExactlyOnce, 1);

        mqtt.handle_incoming_publish(publish).unwrap();
        let (notification, request) = mqtt
            .handle_incoming_pubrel(Pid::try_from(1).unwrap())
            .unwrap();

        match notification {
            None => assert!(true),
            _ => panic!("Invalid notification: {:?}", notification),
        }

        match request {
            Some(Packet::Pubcomp(pid)) => assert_eq!(pid.get(), 1),
            _ => panic!("Invalid network request: {:?}", request),
        }
    }

    #[test]
    fn incoming_pubcomp_should_release_correct_pid_from_release_queue() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];
        let publish = build_outgoing_publish(QoS::ExactlyOnce);

        mqtt.handle_outgoing_publish(publish, buf).unwrap();
        mqtt.handle_incoming_pubrec(Pid::try_from(2).unwrap())
            .unwrap();

        mqtt.handle_incoming_pubcomp(Pid::try_from(2).unwrap())
            .unwrap();
        assert_eq!(mqtt.outgoing_pub.len(), 0);
    }

    #[test]
    fn outgoing_ping_handle_should_throw_errors_for_no_pingresp() {
        let mut mqtt = build_mqttstate();
        let buf = &mut [0u8; 256];
        mqtt.connection_status = MqttConnectionStatus::Connected;
        mqtt.handle_outgoing_ping().unwrap();

        // network activity other than pingresp
        let publish = build_outgoing_publish(QoS::AtLeastOnce);
        mqtt.handle_outgoing_publish(publish.into(), buf).unwrap();
        mqtt.handle_incoming_packet(Packet::Puback(Pid::try_from(2).unwrap()))
            .unwrap();

        // should throw error because we didn't get pingresp for previous ping
        match mqtt.handle_outgoing_ping() {
            Ok(_) => panic!("Should throw pingresp await error"),
            Err(StateError::AwaitPingResp) => (),
            Err(e) => panic!("Should throw pingresp await error. Error = {:?}", e),
        }
    }

    #[test]
    fn outgoing_ping_handle_should_succeed_if_pingresp_is_received() {
        let mut mqtt = build_mqttstate();

        mqtt.connection_status = MqttConnectionStatus::Connected;

        // should ping
        mqtt.handle_outgoing_ping().unwrap();
        mqtt.handle_incoming_packet(Packet::Pingresp).unwrap();

        // should ping
        mqtt.handle_outgoing_ping().unwrap();
    }
}
