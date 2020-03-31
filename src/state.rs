use crate::{Notification, PublishRequest, Request, Subscribe, SubscribeRequest};
use alloc::collections::vec_deque::VecDeque;
use mqttrs::*;

#[allow(unused)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttConnectionStatus {
    Handshake,
    Connected,
    Disconnecting,
    Disconnected,
}

#[derive(Debug)]
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
}

/// State of the mqtt connection.
// Methods will just modify the state of the object without doing any network operations
// This abstracts the functionality better so that it's easy to switch between synchronous code,
// tokio (or) async/await
#[derive(Debug, Clone)]
pub struct MqttState<I, O> {
    /// Connection status
    pub connection_status: MqttConnectionStatus,
    /// Status of last ping
    pub await_pingresp: bool,
    /// Last incoming packet time
    pub last_incoming_timer: I,
    /// Last outgoing packet time
    pub last_outgoing_timer: O,
    /// Packet id of the last outgoing packet
    pub last_pid: Pid,
    // Outgoing QoS 1, 2 publishes which aren't acked yet
    pub outgoing_pub: VecDeque<Publish>,
    // /// Packet ids of released QoS 2 publishes
    pub outgoing_rel: VecDeque<Pid>,
    // /// Packet ids on incoming QoS 2 publishes
    pub incoming_pub: VecDeque<Pid>,
}

impl<I, O> MqttState<I, O>
where
    I: embedded_hal::timer::CountDown,
    O: embedded_hal::timer::CountDown,
    I::Time: From<u32>,
    O::Time: From<u32>,
{
    /// Creates new mqtt state. Same state should be used during a
    /// connection for persistent sessions while new state should
    /// instantiated for clean sessions
    pub fn new(incoming_timer: I, outgoing_timer: O) -> Self {
        MqttState {
            connection_status: MqttConnectionStatus::Disconnected,
            await_pingresp: false,
            last_incoming_timer: incoming_timer,
            last_outgoing_timer: outgoing_timer,
            last_pid: Pid::new(),

            // TODO: Change these to heapless::LinearMaps?
            outgoing_pub: VecDeque::new(),
            outgoing_rel: VecDeque::new(),
            incoming_pub: VecDeque::new(),
        }
    }

    /// Consolidates handling of all outgoing mqtt packet logic. Returns a packet which should
    /// be put on to the network by the eventloop
    pub(crate) fn handle_outgoing_packet(
        &mut self,
        packet: Packet,
        timeout_ms: u32,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        let out = match packet {
            // Packet::Publish(publish) => self.handle_outgoing_publish(publish, timeout_ms)?,
            Packet::Pingreq => self.handle_outgoing_ping(timeout_ms)?,
            _ => unimplemented!(),
        };

        Ok((None, Some(out)))
    }

    /// Consolidates handling of all outgoing mqtt packet logic. Returns a packet which should
    /// be put on to the network by the eventloop
    pub(crate) fn handle_outgoing_request(
        &mut self,
        request: Request,
        timeout_ms: u32,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        let out = match request {
            Request::Publish(publish) => self.handle_outgoing_publish(publish, timeout_ms)?,
            Request::Subscribe(subscribe) => {
                self.handle_outgoing_subscribe(subscribe, timeout_ms)?
            }
            _ => unimplemented!(),
        };

        Ok((None, Some(out)))
    }

    /// Consolidates handling of all incoming mqtt packets. Returns a `Notification` which for the
    /// user to consume and `Packet` which for the eventloop to put on the network
    /// E.g For incoming QoS1 publish packet, this method returns (Publish, Puback). Publish packet will
    /// be forwarded to user and Pubck packet will be written to network
    pub(crate) fn handle_incoming_packet(
        &mut self,
        packet: Packet,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        let out = match packet {
            Packet::Pingresp => self.handle_incoming_pingresp(),
            Packet::Publish(publish) => self.handle_incoming_publish(publish.clone()),
            Packet::Suback(suback) => self.handle_incoming_suback(suback),
            Packet::Unsuback(pid) => self.handle_incoming_unsuback(pid),
            Packet::Puback(pid) => self.handle_incoming_puback(pid),
            Packet::Pubrec(pid) => self.handle_incoming_pubrec(pid),
            Packet::Pubrel(pid) => self.handle_incoming_pubrel(pid),
            Packet::Pubcomp(pid) => self.handle_incoming_pubcomp(pid),
            _ => {
                log::error!("Invalid incoming paket = {:?}", packet);
                Ok((None, None))
            }
        };

        // self.last_incoming = Instant::now();
        out
    }

    /// Adds next packet identifier to QoS 1 and 2 publish packets and returns
    /// it by wrapping publish in packet
    fn handle_outgoing_publish(
        &mut self,
        publish: PublishRequest,
        timeout_ms: u32,
    ) -> Result<Packet, StateError> {
        self.last_outgoing_timer.start(timeout_ms);

        let qospid = match publish.qos {
            QoS::AtMostOnce => QosPid::AtMostOnce,
            QoS::AtLeastOnce => QosPid::AtLeastOnce(self.next_pid()),
            QoS::ExactlyOnce => QosPid::ExactlyOnce(self.next_pid()),
        };

        let publish = Publish {
            dup: publish.dup,
            qospid,
            retain: publish.retain,
            topic_name: publish.topic_name,
            payload: publish.payload,
        };

        self.outgoing_pub.push_back(publish.clone());

        log::trace!(
            "Publish. Topic = {:?}, pid = {:?}, Payload Size = {:?}",
            publish.topic_name,
            publish.qospid,
            publish.payload.len()
        );

        Ok(publish.into())
    }

    /// Iterates through the list of stored publishes and removes the publish with the
    /// matching packet identifier. Removal is now a O(n) operation. This should be
    /// usually ok in case of acks due to ack ordering in normal conditions. But in cases
    /// where the broker doesn't guarantee the order of acks, the performance won't be optimal
    fn handle_incoming_puback(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        match self.outgoing_pub.iter().position(|x| {
            x.qospid == QosPid::AtLeastOnce(pid) || x.qospid == QosPid::ExactlyOnce(pid)
        }) {
            Some(index) => {
                let _publish = self.outgoing_pub.remove(index).expect("Wrong index");

                let request = None;
                let notification = Some(Notification::Puback(pid));
                Ok((notification, request))
            }
            None => {
                log::error!("Unsolicited puback packet: {:?}", pid);
                Err(StateError::Unsolicited)
            }
        }
    }

    fn handle_incoming_suback(
        &mut self,
        suback: Suback,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        let request = None;
        let notification = Some(Notification::Suback(suback));
        Ok((notification, request))
    }

    fn handle_incoming_unsuback(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
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
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        match self.outgoing_pub.iter().position(|x| {
            x.qospid == QosPid::AtLeastOnce(pid) || x.qospid == QosPid::ExactlyOnce(pid)
        }) {
            Some(index) => {
                let _ = self.outgoing_pub.remove(index);
                self.outgoing_rel.push_back(pid);

                let reply = Some(Packet::Pubrel(pid));
                let notification = Some(Notification::Pubrec(pid));
                Ok((notification, reply))
            }
            None => {
                log::error!("Unsolicited pubrec packet: {:?}", pid);
                Err(StateError::Unsolicited)
            }
        }
    }

    /// Results in a publish notification in all the QoS cases. Replys with an ack
    /// in case of QoS1 and Replys rec in case of QoS while also storing the message
    fn handle_incoming_publish(
        &mut self,
        publish: Publish,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        let qospid = publish.qospid;

        match qospid {
            QosPid::AtMostOnce => {
                let notification = Notification::Publish(publish);
                Ok((Some(notification), None))
            }
            QosPid::AtLeastOnce(pid) => {
                let request = Packet::Puback(pid);
                let notification = Notification::Publish(publish);
                Ok((Some(notification), Some(request)))
            }
            QosPid::ExactlyOnce(pid) => {
                let reply = Packet::Pubrec(pid);
                let notification = Notification::Publish(publish);

                self.incoming_pub.push_back(pid);
                Ok((Some(notification), Some(reply)))
            }
        }
    }

    fn handle_incoming_pubrel(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        match self.incoming_pub.iter().position(|x| *x == pid) {
            Some(index) => {
                let _ = self.incoming_pub.remove(index);
                let reply = Packet::Pubcomp(pid);
                Ok((None, Some(reply)))
            }
            None => {
                log::error!("Unsolicited pubrel packet: {:?}", pid);
                Err(StateError::Unsolicited)
            }
        }
    }

    fn handle_incoming_pubcomp(
        &mut self,
        pid: Pid,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        match self.outgoing_rel.iter().position(|x| *x == pid) {
            Some(index) => {
                self.outgoing_rel.remove(index).expect("Wrong index");
                let notification = Some(Notification::Pubcomp(pid));
                let reply = None;
                Ok((notification, reply))
            }
            _ => {
                log::error!("Unsolicited pubcomp packet: {:?}", pid);
                Err(StateError::Unsolicited)
            }
        }
    }

    /// check when the last control packet/pingreq packet is received and return
    /// the status which tells if keep alive time has exceeded
    /// NOTE: status will be checked for zero keepalive times also
    fn handle_outgoing_ping(&mut self, timeout_ms: u32) -> Result<Packet, StateError> {
        // raise error if last ping didn't receive ack
        if self.await_pingresp {
            log::error!("Error awaiting for last ping response");
            return Err(StateError::AwaitPingResp);
        }

        self.last_outgoing_timer.start(timeout_ms);
        self.await_pingresp = true;

        log::trace!("Pingreq");

        Ok(Packet::Pingreq)
    }

    fn handle_incoming_pingresp(
        &mut self,
    ) -> Result<(Option<Notification>, Option<Packet>), StateError> {
        self.await_pingresp = false;
        log::trace!("Pingresp");
        Ok((None, None))
    }

    fn handle_outgoing_subscribe(
        &mut self,
        subscribe_request: SubscribeRequest,
        timeout_ms: u32,
    ) -> Result<Packet, StateError> {
        self.last_outgoing_timer.start(timeout_ms);

        let subscription = Subscribe {
            pid: self.next_pid(),
            topics: subscribe_request.topics,
        };

        log::trace!(
            "Subscribe. Topics = {:?}, pid = {:?}",
            subscription.topics,
            subscription.pid
        );
        Ok(subscription.into())
    }

    pub fn handle_outgoing_connect(&mut self, timeout_ms: u32) -> Result<(), StateError> {
        self.connection_status = MqttConnectionStatus::Handshake;
        self.last_outgoing_timer.start(timeout_ms);
        Ok(())
    }

    pub fn handle_incoming_connack(&mut self, packet: Packet) -> Result<(), StateError> {
        let connack = match packet {
            Packet::Connack(connack) => connack,
            packet => {
                log::error!("Invalid packet. Expecting connack. Received = {:?}", packet);
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
                log::error!(
                    "Invalid state. Expected = {:?}, Current = {:?}",
                    MqttConnectionStatus::Handshake,
                    self.connection_status
                );
                self.connection_status = MqttConnectionStatus::Disconnected;
                Err(StateError::InvalidState)
            }
            code => {
                log::error!("Connection failed. Connection error = {:?}", code);
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
