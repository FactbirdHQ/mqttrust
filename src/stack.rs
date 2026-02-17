use core::ops::{Deref as _, DerefMut};

use embassy_futures::select::{select3, Either3};
use embassy_sync::mutex::MutexGuard;
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embassy_time::Duration;
use embassy_time::{Instant, Timer};
use embedded_io_async::{Error as _, Read, Write};

use crate::crate_config::MAX_SUBSCRIBERS;

use crate::{
    config::Config,
    encoding::{
        received_packet::ReceivedPacket, Connect, PingReq, Protocol, PubAck, QosPid, StateError,
    },
    error::ConnectionError,
    packet::{write_packet, PacketReader},
    pubsub::{FramePublisher, FrameSubscriber, SliceBufferProvider},
    state::{AckStatus, Shared},
    transport::Transport,
    Disconnect,
};
use crate::{ConnAck, SubAck, UnsubAck};

#[cfg(feature = "mqttv5")]
use crate::{Properties, Property, PubAckReasonCode, QoS};

#[cfg(feature = "qos2")]
use crate::{PubComp, PubRec, PubRel};

/// The MQTT protocol engine that manages connections, keep-alive, and packet I/O.
///
/// This must be driven by calling [`run()`](Self::run) in a long-running async task.
pub struct MqttStack<'a, M: RawMutex> {
    shared: &'a Mutex<M, Shared>,
    tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
    rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, MAX_SUBSCRIBERS>,

    config: Config<'a>,
    packet_reader: PacketReader,
    write_buf: [u8; 32],

    last_network_action: Instant,
    awaiting_pingresp: bool,

    clean_start: bool,
    connect_attempts: u8,
}

impl<'a, M: RawMutex> MqttStack<'a, M> {
    pub(crate) fn new(
        config: Config<'a>,
        shared: &'a Mutex<M, Shared>,
        tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
        rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, MAX_SUBSCRIBERS>,
    ) -> Self {
        Self {
            shared,
            tx_subscriber,
            rx_publisher,

            config,
            packet_reader: PacketReader::new(),
            write_buf: [0u8; 32],

            clean_start: true,
            connect_attempts: 0,

            last_network_action: Instant::now(),
            awaiting_pingresp: false,
        }
    }

    /// Runs the MQTT stack, connecting to the broker and processing packets indefinitely.
    ///
    /// This method handles connection, reconnection with backoff, keep-alive pings,
    /// and dispatching incoming messages to subscribers. It returns only if the
    /// maximum number of connection attempts is exhausted.
    pub async fn run<T: Transport>(&mut self, transport: &mut T) {
        loop {
            if !transport.is_connected() {
                self.shared.lock().await.set_connected(None);

                let result = embassy_time::with_timeout(self.config.connect_timeout, async {
                    debug!("Connecting to MQTT broker...");
                    transport.connect().await?;

                    let socket = transport.socket().map_err(ConnectionError::MqttState)?;
                    self.connect_mqtt(socket).await
                })
                .await;

                match result {
                    Ok(Ok(session_present)) => {
                        self.shared
                            .lock()
                            .await
                            .set_connected(Some(session_present));
                    }
                    _ => {
                        error!("Failed to connect to MQTT broker, retrying...");
                        transport.disconnect().ok();

                        match (self.config.backoff_algo)(self.connect_attempts) {
                            Some(backoff) => {
                                embassy_time::Timer::after(backoff).await;
                            }
                            None => {
                                error!("Max reconnect attempts reached, stopping MQTT stack");
                                return;
                            }
                        };

                        self.connect_attempts += 1;
                        continue;
                    }
                }
            }

            let fut = async {
                let socket = transport.socket()?;
                self.select(socket).await
            };

            if let Err(e) = fut.await {
                error!("Stack error {}", e);
                // Clean state
                transport.disconnect().ok();
                self.reset().await;
            }
        }
    }

    pub async fn reset(&mut self) {
        self.awaiting_pingresp = false;
        self.connect_attempts = 0;
        self.shared.lock().await.reset();
        self.packet_reader.reset();
    }

    pub async fn disconnect<T: Transport>(
        &mut self,
        transport: &mut T,
    ) -> Result<(), ConnectionError> {
        let disconnect = Disconnect::builder();
        #[cfg(feature = "mqttv5")]
        let disconnect =
            disconnect.properties(Properties::Slice(&[Property::SessionExpiryInterval(0)]));

        debug!("Disconnecting from MQTT");
        write_packet(
            &mut self.write_buf,
            transport.socket().map_err(ConnectionError::MqttState)?,
            disconnect.build(),
        )
        .await
        .map_err(ConnectionError::MqttState)?;

        transport.disconnect()?;
        self.shared.lock().await.reset();
        Ok(())
    }

    async fn select<S: Read + Write>(&mut self, socket: &mut S) -> Result<(), StateError> {
        let now = Instant::now();
        let keep_alive_sleep = Self::calculate_keep_alive_sleep(
            self.last_network_action,
            self.config.keepalive_interval,
            now,
        );

        match select3(
            self.tx_subscriber.read_async(),
            self.packet_reader.get_received_packet(socket),
            keep_alive_sleep,
        )
        .await
        {
            Either3::First(Ok(tx_grant)) => {
                // ### TX future:
                socket
                    .write_all(tx_grant.deref())
                    .await
                    .map_err(|e| StateError::Io(e.kind()))?;

                socket.flush().await.map_err(|e| StateError::Io(e.kind()))?;

                let mut shared = self.shared.lock().await;

                let pid = tx_grant.pid();
                if pid != 0 {
                    shared.outgoing_pid.remove(&pid);
                    trace!(
                        "Outgoing PID {} successfully transmitted! Missing tx: {:?}",
                        pid,
                        shared.outgoing_pid.len()
                    );
                }

                tx_grant.release();
                shared.wake_tx();
            }
            Either3::Second(Ok(mut packet)) => {
                // ### RX future:
                //
                // Handle all incoming packet types by sending ack & waking
                // // `tx_wakers`.
                match packet {
                    ReceivedPacket::ConnAck(_) => {
                        // This should never happen, as this function is not
                        // used until after successfully connected to MQTT
                        // broker
                        warn!("Got unexpected connack");
                    }
                    ReceivedPacket::Disconnect(Disconnect { reason_code, .. }) => {
                        warn!("Received disconnect packet {:?}", reason_code);
                    }
                    ReceivedPacket::PingResp => {
                        // If there was no timeout to begin with, log the spurious ping response.
                        if !self.awaiting_pingresp {
                            warn!("Got unexpected ping response");
                        }
                        self.awaiting_pingresp = false;
                    }
                    ReceivedPacket::PartialPublish {
                        qos_pid,
                        topic_name,
                        ref mut publish,
                    } => 'publish: {
                        // Handle incoming `Publish` type packets by copying the full
                        // packet to any `shared.rx_publisher` buffers where topic
                        // matches their topic_filter.

                        trace!("Adding {:?} as available for subscribers", topic_name);

                        match self.rx_publisher.grant_immediate(publish.len()) {
                            Ok(mut grant) => {
                                publish.copy_all(grant.deref_mut()).await?;

                                // calling `commit` will wake all subscribers
                                grant.commit(publish.len());
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to buffer incoming publish ({:?}), discarding payload",
                                    e
                                );
                                publish.discard().await?;
                                // Skip PubAck intentionally — for QoS 1, the broker will
                                // re-deliver when we don't ack. For QoS 0, no ack needed.
                                break 'publish;
                            }
                        }

                        // Write `PubAck` or `PubRec` depending on received QoS
                        match qos_pid {
                            QosPid::AtMostOnce => {}
                            QosPid::AtLeastOnce(pid) => {
                                let puback = PubAck {
                                    pid,
                                    #[cfg(feature = "mqttv5")]
                                    reason_code: PubAckReasonCode::Success,
                                    #[cfg(feature = "mqttv5")]
                                    properties: Properties::Slice(&[]),
                                    #[cfg(feature = "mqttv3")]
                                    _marker: core::marker::PhantomData,
                                };
                                write_packet(&mut self.write_buf, socket, puback).await?;
                            }
                            #[cfg(feature = "qos2")]
                            QosPid::ExactlyOnce(pid) => {
                                self.shared
                                    .lock()
                                    .await
                                    .incoming_pub
                                    .insert(pid.get())
                                    .unwrap();

                                let pubrec = PubRec { pid };
                                write_packet(&mut self.write_buf, socket, pubrec).await?;
                            }
                        }
                    }
                    ReceivedPacket::PubAck(p) => {
                        let shared = self.shared.lock().await;
                        Self::handle_pub_ack(shared, p)?;
                    }
                    ReceivedPacket::SubAck(SubAck { pid, codes, .. }) => {
                        let shared = self.shared.lock().await;
                        Self::handle_sub_ack(shared, pid, codes)?;
                    }
                    ReceivedPacket::UnsubAck(UnsubAck { pid, codes, .. }) => {
                        let shared = self.shared.lock().await;
                        Self::handle_unsub_ack(shared, pid, codes)?;
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubRel(p) => {
                        let mut shared = self.shared.lock().await;
                        let packet = Self::handle_pub_rel(shared, p)?;
                        write_packet(&mut self.write_buf, socket, packet).await?;
                        shared.wake_tx();
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubRec(p) => {
                        let mut shared = self.shared.lock().await;
                        let packet = Self::handle_pub_rec(shared, p)?;
                        write_packet(&mut self.write_buf, socket, packet).await?;
                        shared.wake_tx();
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubComp(p) => {
                        let mut shared = self.shared.lock().await;
                        Self::handle_pub_comp(shared, p)?;
                    }
                };
            }
            Either3::Second(Err(StateError::Io(e))) => {
                // Non-recoverable socket error that requires us to disconnect the socket and reconnect
                return Err(StateError::Io(e));
            }
            Either3::Third(_) => {
                let packet = self.handle_keep_alive()?;
                write_packet(&mut self.write_buf, socket, packet).await?;
            }
            _ => {}
        }

        // Update the last network action, in order to keep track of when to
        // PING for keep-alive
        self.last_network_action = Instant::now();

        Ok(())
    }

    fn calculate_keep_alive_sleep(last_action: Instant, interval: Duration, now: Instant) -> Timer {
        if let Some(remaining_time) = (last_action + interval).checked_duration_since(now) {
            Timer::after(remaining_time)
        } else {
            Timer::after(Duration::MIN)
        }
    }

    fn handle_keep_alive(&mut self) -> Result<PingReq, StateError> {
        if self.awaiting_pingresp {
            return Err(StateError::AwaitPingResp);
        }

        debug!(
            "Pingreq, last network action @ {} millisecs, current time {} millisecs",
            self.last_network_action.as_millis(),
            Instant::now().as_millis()
        );

        self.awaiting_pingresp = true;

        Ok(PingReq)
    }

    fn handle_pub_ack(
        mut shared: MutexGuard<'_, M, Shared>,
        p: PubAck<'_>,
    ) -> Result<(), StateError> {
        debug!("Removing PID {:?} from inflight_pub", p.pid.get());
        if !shared.inflight_pub.remove(&p.pid.get()) {
            warn!("Unexpected Puback, PID: {:?}", p.pid.get());
        }
        #[cfg(feature = "mqttv5")]
        if p.reason_code != PubAckReasonCode::Success {
            warn!("Received PUBACK with reason code: {:?}", p.reason_code);
        }
        shared.wake_tx();
        Ok(())
    }

    #[cfg(feature = "qos2")]
    fn handle_pub_rel(
        mut shared: MutexGuard<'_, M, Shared>,
        p: PubRel,
    ) -> Result<Packet, StateError> {
        let mut shared = shared.lock().await;
        if shared.incoming_pub.remove(&p.pid.get()) {
            Ok(Packet::PubComp(PubComp { pid: p }))
        } else {
            error!("Unsolicited pubrel packet: {:?}", p.pid);
            Err(StateError::UnexpectedPacket)
        }
    }

    #[cfg(feature = "qos2")]
    fn handle_pub_rec(
        mut shared: MutexGuard<'_, M, Shared>,
        p: PubRec,
    ) -> Result<Packet, StateError> {
        if shared.inflight_pub.remove(&p.pid.get()).is_some() {
            shared.outgoing_rel.insert(p.pid.get());
            Ok(Packet::PubRel(PubRel { pid: p }))
        } else {
            error!("Unsolicited pubrec packet: {:?}", p.pid);
            Err(StateError::UnexpectedPacket)
        }
    }

    #[cfg(feature = "qos2")]
    fn handle_pub_comp(
        mut shared: MutexGuard<'_, M, Shared>,
        p: PubComp,
    ) -> Result<(), StateError> {
        if shared.outgoing_rel.remove(&p.pid.get()) {
            shared.wake_tx();
        } else {
            error!("Unsolicited pubcomp packet: {:?}", p.pid);
        }
        Ok(())
    }

    fn handle_sub_ack(
        mut shared: MutexGuard<'_, M, Shared>,
        pid: crate::Pid,
        codes: &[u8],
    ) -> Result<(), StateError> {
        debug!("Received suback: {:?}, {:?}", pid, codes);
        match shared.ack_status.get_mut(&pid.get()) {
            Some(status) if *status == AckStatus::AwaitingSubAck => {
                *status = AckStatus::Acked(heapless::Vec::from_slice(codes).unwrap());
                shared.wake_tx();
            }
            None | Some(_) => {
                error!("Unsolicited suback packet: {:?}", pid);
            }
        }
        Ok(())
    }

    fn handle_unsub_ack(
        mut shared: MutexGuard<'_, M, Shared>,
        pid: crate::Pid,
        codes: &[u8],
    ) -> Result<(), StateError> {
        match shared.ack_status.get_mut(&pid.get()) {
            Some(status) if matches!(*status, AckStatus::AwaitingUnsubAck(true)) => {
                *status = AckStatus::Acked(heapless::Vec::from_slice(codes).unwrap());
            }
            Some(status) if matches!(*status, AckStatus::AwaitingUnsubAck(false)) => {
                shared.ack_status.remove(&pid.get());
            }
            None | Some(_) => {
                error!("Unsolicited suback packet: {:?}", pid);
                return Ok(());
            }
        }
        shared.wake_tx();
        Ok(())
    }

    fn handle_connack(p: ConnAck<'_>, _config: &mut Config<'_>) -> Result<bool, ConnectionError> {
        #[cfg(feature = "mqttv5")]
        for prop in p.properties.iter() {
            match prop {
                Ok(Property::ServerKeepAlive(keep_alive)) => {
                    _config.keepalive_interval = _config
                        .keepalive_interval
                        .min(Duration::from_secs(keep_alive as u64));
                }
                Ok(Property::MaximumQoS(qos)) => {
                    _config.max_qos = _config
                        .max_qos
                        .max(QoS::try_from(qos).unwrap_or(QoS::AtLeastOnce));
                }
                Ok(Property::ReasonString(reason)) => {
                    error!(" => {}", reason);
                }
                _ => (),
            }
        }

        if p.reason_code.success() {
            Ok(p.session_present)
        } else {
            error!("Connection refused! reason code: {:?}", p.reason_code);
            Err(ConnectionError::ConnectionRefused)
        }
    }

    async fn connect_mqtt<S: Read + Write>(
        &mut self,
        socket: &mut S,
    ) -> Result<bool, ConnectionError> {
        let connect = Connect {
            #[cfg(feature = "mqttv3")]
            protocol: Protocol::MQTT311,
            #[cfg(feature = "mqttv5")]
            protocol: Protocol::MQTT5,
            keep_alive: self.config.keepalive_interval.as_secs() as u16,
            client_id: &self.config.client_id,
            clean_start: self.clean_start,

            last_will: self.config.last_will.clone(),
            username: self.config.username,
            password: self.config.password,
            #[cfg(feature = "mqttv5")]
            properties: Properties::Slice(&[Property::SessionExpiryInterval(600)]),
        };

        if self.clean_start {
            debug!("Connecting to MQTT with options: {:?}", connect);
        } else {
            debug!("Reconnecting to MQTT with options: {:?}", connect);
        }

        // send mqtt connect packet — use a stack buffer large enough for
        // client_id + username + password + last_will
        let mut connect_buf = [0u8; 512];
        write_packet(&mut connect_buf, socket, connect)
            .await
            .map_err(ConnectionError::MqttState)?;

        self.last_network_action = Instant::now();

        let packet = self
            .packet_reader
            .get_received_packet(socket)
            .await
            .map_err(|_| ConnectionError::FlushTimeout)?;
        // validate connack
        let result = match packet {
            ReceivedPacket::ConnAck(p) => Self::handle_connack(p, &mut self.config),
            _ => Err(ConnectionError::NotConnAck),
        };

        match result {
            Ok(present) => {
                if self.clean_start {
                    debug!("Connected! Reusing existing session: {}", present);
                } else {
                    debug!("Reconnected! Reusing existing session: {}", present);
                }

                self.clean_start = false;
                self.connect_attempts = 0;

                Ok(present)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embedded_nal_async::TcpConnect;
    use futures_util::StreamExt;
    use static_cell::StaticCell;

    use crate::{
        broker::IpBroker,
        config::Config,
        encoding::{Publish, QoS, Subscribe},
        State,
    };

    struct MockNetwork;

    impl TcpConnect for MockNetwork {
        type Error = Infallible;

        type Connection<'a>
            = MockSocket
        where
            Self: 'a;

        async fn connect(
            &self,
            _remote: core::net::SocketAddr,
        ) -> Result<Self::Connection<'_>, Self::Error> {
            Ok(MockSocket)
        }
    }

    struct MockSocket;

    impl embedded_io_async::ErrorType for MockSocket {
        type Error = Infallible;
    }

    impl embedded_io_async::Read for MockSocket {
        async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, Self::Error> {
            Ok(0)
        }
    }

    impl embedded_io_async::Write for MockSocket {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
            Ok(buf.len())
        }
    }

    #[cfg(feature = "mqttv5")]
    #[tokio::test]
    #[ignore = "Skipped for now"]
    async fn subscribe_publish() {
        let mut network = crate::transport::embedded_nal::NalTransport::new(
            &MockNetwork,
            IpBroker::new(core::net::Ipv4Addr::LOCALHOST, 1883),
        );

        // Create the MQTT stack
        static STATE: StaticCell<State<NoopRawMutex, 4096, 4096>> = StaticCell::new();
        let state = STATE.init(State::<NoopRawMutex, 4096, 4096>::new());
        let config = Config::builder()
            .client_id("client_id".try_into().unwrap())
            .build();
        let (mut stack, client) = crate::new(state, config);

        let fut = async {
            // Use the MQTT client to subscribe
            let topics = ["ABC".into()];
            let subscribe = Subscribe::builder().topics(&topics).build();

            let mut subscription = client.subscribe::<1>(subscribe).await.unwrap();

            client
                .publish(Publish::builder().topic_name("ABC").payload(&[]).build())
                .await
                .unwrap();

            while let Some(message) = subscription.next().await {
                if let "ABC" = message.topic_name() {
                    client
                        .publish(Publish::builder().topic_name("ABC").payload(&[]).build())
                        .await
                        .unwrap();
                    break;
                }
            }
        };

        embassy_futures::select::select(stack.run(&mut network), fut).await;
    }

    #[cfg(feature = "mqttv5")]
    #[tokio::test]
    #[ignore = "Skipped for now"]
    async fn multiple_subscribe() {
        let mut network = crate::transport::embedded_nal::NalTransport::new(
            &MockNetwork,
            IpBroker::new(core::net::Ipv4Addr::LOCALHOST, 1883),
        );

        // Create the MQTT stack
        static STATE: StaticCell<State<NoopRawMutex, 4096, 4096>> = StaticCell::new();
        let state = STATE.init(State::<NoopRawMutex, 4096, 4096>::new());
        let config = Config::builder()
            .client_id("client_id".try_into().unwrap())
            .build();
        let (mut stack, client) = crate::new(state, config);

        // Use the MQTT client to subscribe

        client
            .publish(Publish::builder().topic_name("ABC").payload(&[]).build())
            .await
            .unwrap();

        let fut_a = async {
            let topics = ["ABC".into()];
            let subscribe_a = Subscribe::builder().topics(&topics).build();

            let mut subscription_a = client.subscribe::<1>(subscribe_a).await.unwrap();
            while let Some(message) = subscription_a.next().await {
                if let "ABC" = message.topic_name() {
                    client
                        .publish(Publish {
                            dup: false,
                            qos: QoS::AtLeastOnce,
                            pid: None,
                            retain: false,
                            topic_name: "CDE",
                            payload: b"",
                            properties: crate::Properties::Slice(&[]),
                        })
                        .await
                        .unwrap();
                    break;
                }
            }
        };

        let fut_b = async {
            let topics = ["CDE".into()];
            let subscribe_b = Subscribe::builder().topics(&topics).build();

            let mut subscription_b = client.subscribe::<1>(subscribe_b).await.unwrap();

            while let Some(message) = subscription_b.next().await {
                if let "CDE" = message.topic_name() {
                    client
                        .publish(Publish::builder().topic_name("ABC").payload(&[]).build())
                        .await
                        .unwrap();
                    break;
                }
            }
        };

        embassy_futures::select::select(
            stack.run(&mut network),
            embassy_futures::join::join(fut_a, fut_b),
        )
        .await;
    }
}
