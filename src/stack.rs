use core::{
    cell::RefCell,
    ops::{Deref, DerefMut},
};

use embassy_futures::select::{select3, Either3};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embassy_time::{Instant, Timer};
use embedded_io_async::Error as _;
use embedded_io_async::Write as _;

use crate::{
    config::Config,
    encoder::TxHeader,
    encoding::{
        received_packet::ReceivedPacket, Connect, PacketType, PingReq, Protocol, PubAck, QoS,
        QosPid, StateError,
    },
    error::ConnectionError,
    packet::PacketBuffer,
    pubsub::{
        framed::{FramePublisher, FrameSubscriber},
        SliceBufferProvider,
    },
    state::{Inflight, PendingAck, Shared},
    transport::Transport,
    Broker, Disconnect,
};

#[cfg(feature = "qos2")]
use crate::encoding::{PubComp, PubRec, PubRel};

pub struct MqttStack<'a, M: RawMutex, B, const SUBS: usize> {
    shared: &'a Mutex<M, RefCell<Shared<SUBS>>>,
    tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
    rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, SUBS>,

    config: Config<B>,
    packet_buf: PacketBuffer<128>,

    last_network_action: Instant,
    await_pingresp: Option<Instant>,

    clean_start: bool,
    connect_attempts: u8,
    should_connect: bool,
}

impl<'a, M: RawMutex, B: Broker, const SUBS: usize> MqttStack<'a, M, B, SUBS> {
    pub(crate) fn new(
        config: Config<B>,
        shared: &'a Mutex<M, RefCell<Shared<SUBS>>>,
        tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
        rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, SUBS>,
    ) -> Self {
        Self {
            shared,
            tx_subscriber,
            rx_publisher,

            config,
            packet_buf: PacketBuffer::new(),

            clean_start: true,
            connect_attempts: 0,
            should_connect: true,

            last_network_action: Instant::now(),
            await_pingresp: None,
        }
    }

    // FIXME: This is currently not able to change between `Broker` types at
    // runtime, meaning if `Stack` is insatantiated with a `DomainBroker` it is
    // only possible to update the config to a new `DomainBroker` implementation
    // with the same `Dns` resolver.
    pub fn update_config<F: FnOnce(&mut Config<B>)>(&mut self, f: F) {
        f(&mut self.config);
    }

    pub async fn run(&mut self, transport: &mut impl Transport) {
        self.should_connect = true;
        info!("Running stack!");
        while self.should_connect {
            if !transport.is_connected() {
                match embassy_time::with_timeout(self.config.connect_timeout, async {
                    transport.connect(&mut self.config.broker).await?;

                    self.connect_mqtt(transport).await?;
                    Ok::<_, ConnectionError>(())
                })
                .await
                {
                    Ok(_) => {}
                    Err(_) => {
                        transport.disconnect();
                        embassy_time::Timer::after((self.config.backoff_algo)(
                            self.connect_attempts,
                        ))
                        .await;
                        self.connect_attempts += 1;
                        continue;
                    }
                }
            }

            if let Err(e) = self.select(transport).await {
                error!("Stack error {}", e);
                // Clean state
            }
        }

        let disconnect = Disconnect {
            reason_code: Default::default(),
            #[cfg(feature = "mqttv5")]
            properties: crate::Properties::Slice(&[]),
        };

        self.packet_buf
            .write_packet(transport, disconnect)
            .await
            .ok();

        self.last_network_action = Instant::now();
    }

    pub async fn disconnect(&mut self) -> Result<(), ConnectionError> {
        self.should_connect = false;
        Ok(())
    }

    async fn select(&mut self, transport: &mut impl Transport) -> Result<(), StateError> {
        let keep_alive_sleep = if let Some(instant) = self.await_pingresp {
            Timer::after(instant + self.config.keepalive_interval - Instant::now())
        } else {
            Timer::after(self.last_network_action + self.config.keepalive_interval - Instant::now())
        };

        match select3(
            self.packet_buf.get_received_packet(transport),
            self.tx_subscriber.read_async(),
            keep_alive_sleep,
        )
        .await
        {
            Either3::First(Ok(mut packet)) => {
                // ### RX future:
                //
                // Handle to all incoming packet types by sending ack & waking
                // `tx_wakers`.
                self.last_network_action = Instant::now();

                match packet {
                    ReceivedPacket::Disconnect { reason_code, .. } => {
                        warn!("Received disconnect packet {}", reason_code);
                    }
                    ReceivedPacket::PingResp => {
                        // If there was no timeout to begin with, log the spurious ping response.
                        if self.await_pingresp.take().is_none() {
                            warn!("Got unexpected ping response");
                        }
                    }
                    ReceivedPacket::ConnAck { .. } => {
                        // This should never happen, as this function is not used until after successfully connected to MQTT broker
                        warn!("Got unexpected connack");
                    }
                    ReceivedPacket::Publish {
                        qos_pid,
                        ref mut publish,
                    } => {
                        // Handle incoming `Publish` type packets by copying the full
                        // packet to any `shared.rx_publisher` buffers where topic
                        // matches their topic_filter.
                        match self.rx_publisher.grant_async(publish.len()).await {
                            Ok(mut grant) => {
                                // FIXME: Properly handle error instead of `unwrap`
                                publish.copy_all(grant.deref_mut()).await.unwrap();

                                // calling `commit` will wake all subscribers
                                grant.commit(publish.len());
                            }
                            Err(_) => {
                                error!(
                                    "Packet is larger than the storage allocated in `rx_publisher`"
                                );
                            }
                        }

                        // Write `PubAck` or `PubRec` depending on received QoS
                        match qos_pid {
                            QosPid::AtMostOnce => {}
                            QosPid::AtLeastOnce(pid) => {
                                warn!("sending puback {:?}", pid);
                                let puback = PubAck { pid };
                                self.packet_buf.write_packet(transport, puback).await?;
                            }
                            #[cfg(feature = "qos2")]
                            QosPid::ExactlyOnce(pid) => {
                                self.shared
                                    .lock()
                                    .await
                                    .borrow_mut()
                                    .incoming_pub
                                    .insert(pid.get())
                                    .unwrap();

                                let pubrec = PubRec { pid };
                                self.packet_buf.write_packet(transport, pubrec).await?;
                            }
                        }
                    }
                    ReceivedPacket::PubAck { pid, .. } => {
                        let shared_ref = self.shared.lock().await;
                        let mut shared = shared_ref.borrow_mut();
                        debug!("Removing PID {:?} from outgoing_pub", pid.get());
                        if shared.outgoing_pub.remove(&pid.get()).is_none() {
                            warn!("Unexpected Puback, PID: {:?}", pid.get());
                        }
                        // TODO: Handle collisions (See rumqttc state.rs)
                        shared.wake_tx();
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubRel { pid, .. } => {
                        let shared_ref = self.shared.lock().await;
                        let mut shared = shared_ref.borrow_mut();
                        match shared.incoming_pub.remove(&pid.get()) {
                            true => {
                                let pubcomp = PubComp { pid };
                                self.packet_buf.write_packet(transport, pubcomp).await?;
                            }
                            false => {
                                error!("Unsolicited pubrel packet: {:?}", pid);
                                return Err(StateError::Unsolicited(pid.get()));
                            }
                        }
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubRec { pid, .. } => {
                        let shared_ref = self.shared.lock().await;
                        let mut shared = shared_ref.borrow_mut();
                        match shared.outgoing_pub.remove(&pid.get()) {
                            Some(_) => {
                                shared.outgoing_rel.insert(pid.get());

                                let pubrel = PubRel { pid };
                                self.packet_buf.write_packet(transport, pubrel).await?;
                            }
                            None => {
                                error!("Unsolicited pubrec packet: {:?}", pid);
                                return Err(StateError::Unsolicited(pid.get()));
                            }
                        }
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubComp { pid, .. } => {
                        // TODO: Handle collisions (See rumqttc state.rs)

                        let shared_ref = self.shared.lock().await;
                        let mut shared = shared_ref.borrow_mut();
                        match shared.outgoing_rel.remove(&pid.get()) {
                            true => {
                                // self.inflight -= 1;
                            }
                            false => {
                                error!("Unsolicited pubcomp packet: {:?}", pid);
                                return Err(StateError::Unsolicited(pid.get()));
                            }
                        }
                    }
                    ReceivedPacket::SubAck { pid, .. } => {
                        let shared_ref = self.shared.lock().await;
                        let mut shared = shared_ref.borrow_mut();
                        // Pop pid from pending_ack
                        debug!("Received suback: {:?}", pid);

                        if !shared.pending_ack.remove(&PendingAck::Subscribe(pid.get())) {
                            error!("Unsolicited suback packet: {:?}", pid);
                            return Err(StateError::Unsolicited(pid.get()));
                        }
                        shared.wake_tx();
                    }
                    ReceivedPacket::UnsubAck { pid, .. } => {
                        let shared_ref = self.shared.lock().await;
                        let mut shared = shared_ref.borrow_mut();
                        // Pop pid from pending_ack
                        if !shared
                            .pending_ack
                            .remove(&PendingAck::Unsubscribe(pid.get()))
                        {
                            error!("Unsolicited unsuback packet: {:?}", pid);
                            return Err(StateError::Unsolicited(pid.get()));
                        }
                        shared.wake_tx();
                    }
                }
            }
            Either3::Second(Ok(mut tx_grant)) => {
                tx_grant.auto_release(true);
                // ### TX future:
                // Based on packet QoS, add PID to state & full packet to
                // retry buffer, before writing the packet to network
                let (tx_header, packet_bytes) = TxHeader::from_bytes(tx_grant.deref());

                let shared_ref = self.shared.lock().await;
                let mut shared = shared_ref.borrow_mut();

                match tx_header.typ {
                    PacketType::Publish => {
                        if tx_header.qos != Some(QoS::AtMostOnce) {
                            debug!("[Publish] Inserting {:?} into outgoing_pub", tx_header.pid);
                            // FIXME: Properly handle error instead of `unwrap`
                            shared
                                .outgoing_pub
                                .insert(tx_header.pid.unwrap().get(), Inflight::new(packet_bytes))
                                .unwrap();
                        }
                    }
                    PacketType::Subscribe => {
                        debug!("[Subscribe] Inserting {:?} into pending_ack", tx_header.pid);
                        // FIXME: Properly handle error instead of `unwrap`
                        shared
                            .pending_ack
                            .insert(PendingAck::Subscribe(tx_header.pid.unwrap().get()))
                            .unwrap();
                    }
                    PacketType::Unsubscribe => {
                        debug!(
                            "[Unsubscribe] Inserting {:?} into pending_ack",
                            tx_header.pid
                        );
                        // FIXME: Properly handle error instead of `unwrap`
                        shared
                            .pending_ack
                            .insert(PendingAck::Unsubscribe(tx_header.pid.unwrap().get()))
                            .unwrap();
                    }
                    e => {
                        // Request has invalid header? This should never be possible!
                        // Just log an error, drop the full request packet, and continue handling next packet

                        error!("TX Packet has invalid header?! Dropping packet {:?}", e);
                        return Ok(());
                    }
                }

                transport
                    .socket()?
                    .write_all(packet_bytes)
                    .await
                    .map_err(|e| StateError::Io(e.kind()))?;

                transport
                    .socket()?
                    .flush()
                    .await
                    .map_err(|e| StateError::Io(e.kind()))?;

                shared.wake_tx();
                self.last_network_action = Instant::now();
            }
            Either3::Third(_) => {
                // ### PING future:

                // raise error if last ping didn't receive ack
                if self.await_pingresp.is_some() {
                    return Err(StateError::AwaitPingResp);
                }

                debug!(
                    "Pingreq,
                    last network action @ {} millisecs,
                    current time {} millisecs",
                    self.last_network_action.as_millis(),
                    Instant::now().as_millis()
                );

                let pingreq = PingReq;
                self.packet_buf.write_packet(transport, pingreq).await?;

                self.last_network_action = Instant::now();
                self.await_pingresp = Some(Instant::now());
            }
            _ => {}
        }
        Ok(())
    }

    async fn connect_mqtt(
        &mut self,
        transport: &mut impl Transport,
    ) -> Result<bool, ConnectionError> {
        let connect = Connect {
            #[cfg(feature = "mqttv3")]
            protocol: Protocol::MQTT311,
            #[cfg(feature = "mqttv5")]
            protocol: Protocol::MQTT5,
            keep_alive: self.config.keepalive_interval.as_secs() as u16,
            client_id: &self.config.client_id,
            clean_start: self.clean_start,

            // TODO:
            last_will: None,
            username: None,
            password: None,
            #[cfg(feature = "mqttv5")]
            properties: crate::encoding::Properties::Slice(&[]),
        };

        if self.clean_start {
            debug!("Connecting to MQTT with options: {:?}", connect);
        } else {
            debug!("Reconnecting to MQTT with options: {:?}", connect);
        }

        // send mqtt connect packet
        self.packet_buf
            .write_packet(transport, connect)
            .await
            .map_err(|e| ConnectionError::MqttState(e))?;

        self.last_network_action = Instant::now();

        // validate connack
        // TODO: ERROR types
        match self
            .packet_buf
            .get_received_packet(transport)
            .await
            .map_err(|_| ConnectionError::FlushTimeout)?
        {
            ReceivedPacket::ConnAck {
                reason_code,
                session_present,
                ..
            } if reason_code.success() => {
                if self.clean_start {
                    debug!("Connected! Reusing existing session: {}", session_present);
                } else {
                    debug!("Reconnected! Reusing existing session: {}", session_present);
                }

                // TODO: If the Server returns a Server Keep Alive on the
                // CONNACK packet, the Client MUST use that value instead of the
                // value it sent as the Keep Alive
                self.clean_start = false;
                self.connect_attempts = 0;

                Ok(session_present)
            }
            ReceivedPacket::ConnAck { reason_code, .. } => {
                debug!("Connection refused! reason: {:?}", reason_code);
                Err(ConnectionError::ConnectionRefused)
            }
            _ => Err(ConnectionError::NotConnAck),
        }
    }
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embedded_nal_async::{Ipv4Addr, TcpConnect};
    use futures::StreamExt;
    use static_cell::StaticCell;

    use crate::{
        broker::IpBroker,
        config::Config,
        encoding::{Publish, QoS, Subscribe, SubscribeTopic},
        State,
    };

    struct MockNetwork;

    impl TcpConnect for MockNetwork {
        type Error = Infallible;

        type Connection<'a>
	         = MockSocket where Self: 'a;

        async fn connect<'a>(
            &'a self,
            _remote: embedded_nal_async::SocketAddr,
        ) -> Result<Self::Connection<'a>, Self::Error> {
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
        let mut network = crate::transport::embedded_nal::NalTransport::new(&MockNetwork);

        // Create the MQTT stack
        static STATE: StaticCell<State<NoopRawMutex, 4096, 4096, 4>> = StaticCell::new();
        let state = STATE.init(State::<NoopRawMutex, 4096, 4096, 4>::new());
        let config = Config::new("client_id", IpBroker::new(Ipv4Addr::LOCALHOST, 1883));
        let (mut stack, client) = crate::new(state, config);

        let fut = async {
            // Use the MQTT client to subscribe

            let subscribe = Subscribe::new(&[SubscribeTopic {
                topic_path: "ABC",
                maximum_qos: QoS::AtLeastOnce,
                no_local: false,
                retain_as_published: false,
                retain_handling: crate::RetainHandling::SendAtSubscribeTimeIfNonexistent,
            }]);

            let mut subscription = client.subscribe::<1>(subscribe).await.unwrap();

            client
                .publish(Publish {
                    dup: false,
                    qos: QoS::AtLeastOnce,
                    pid: None,
                    retain: false,
                    topic_name: "ABC",
                    payload: b"",
                    properties: crate::encoding::Properties::Slice(&[]),
                })
                .await
                .unwrap();

            while let Some(message) = subscription.next().await {
                match message.topic_name() {
                    "ABC" => {
                        client
                            .publish(Publish {
                                dup: false,
                                qos: QoS::AtLeastOnce,
                                pid: None,
                                retain: false,
                                topic_name: "ABC",
                                payload: b"",
                                properties: crate::encoding::Properties::Slice(&[]),
                            })
                            .await
                            .unwrap();
                        break;
                    }
                    _ => {}
                }
            }
        };

        embassy_futures::select::select(stack.run(&mut network), fut).await;
    }

    #[cfg(feature = "mqttv5")]
    #[tokio::test]
    #[ignore = "Skipped for now"]
    async fn multiple_subscribe() {
        let mut network = crate::transport::embedded_nal::NalTransport::new(&MockNetwork);

        // Create the MQTT stack
        static STATE: StaticCell<State<NoopRawMutex, 4096, 4096, 4>> = StaticCell::new();
        let state = STATE.init(State::<NoopRawMutex, 4096, 4096, 4>::new());
        let config = Config::new("client_id", IpBroker::new(Ipv4Addr::LOCALHOST, 1883));
        let (mut stack, client) = crate::new(state, config);

        // Use the MQTT client to subscribe

        client
            .publish(Publish {
                dup: false,
                qos: QoS::AtLeastOnce,
                pid: None,
                retain: false,
                topic_name: "ABC",
                payload: b"",
                properties: crate::encoding::Properties::Slice(&[]),
            })
            .await
            .unwrap();

        let fut_a = async {
            let subscribe_a = Subscribe::new(&[SubscribeTopic {
                topic_path: "ABC",
                maximum_qos: QoS::AtLeastOnce,
                no_local: false,
                retain_as_published: false,
                retain_handling: crate::RetainHandling::SendAtSubscribeTimeIfNonexistent,
            }]);

            let mut subscription_a = client.subscribe::<1>(subscribe_a).await.unwrap();
            while let Some(message) = subscription_a.next().await {
                match message.topic_name() {
                    "ABC" => {
                        client
                            .publish(Publish {
                                dup: false,
                                qos: QoS::AtLeastOnce,
                                pid: None,
                                retain: false,
                                topic_name: "CDE",
                                payload: b"",
                                properties: crate::encoding::Properties::Slice(&[]),
                            })
                            .await
                            .unwrap();
                        break;
                    }
                    _ => {}
                }
            }
        };

        let fut_b = async {
            let subscribe_b = Subscribe::new(&[SubscribeTopic {
                topic_path: "CDE",
                maximum_qos: QoS::AtLeastOnce,
                no_local: false,
                retain_as_published: false,
                retain_handling: crate::RetainHandling::SendAtSubscribeTimeIfNonexistent,
            }]);

            let mut subscription_b = client.subscribe::<1>(subscribe_b).await.unwrap();

            while let Some(message) = subscription_b.next().await {
                match message.topic_name() {
                    "CDE" => {
                        client
                            .publish(Publish {
                                dup: false,
                                qos: QoS::AtLeastOnce,
                                pid: None,
                                retain: false,
                                topic_name: "ABC",
                                payload: b"",
                                properties: crate::encoding::Properties::Slice(&[]),
                            })
                            .await
                            .unwrap();
                        break;
                    }
                    _ => {}
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
