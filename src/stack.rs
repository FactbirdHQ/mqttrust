use core::{
    cell::RefCell,
    ops::{Deref, DerefMut},
};

use embassy_futures::select::{select3, Either3};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embassy_time::{Instant, Timer};
use embedded_io_async::{Error as _, ErrorKind, Write};
use embedded_nal_async::TcpConnect;

use crate::{
    config::Config,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        received_packet::ReceivedPacket,
        Connect, PacketType, PingReq, Protocol, PubAck, QoS, QosPid, StateError,
    },
    error::ConnectionError,
    packet::PacketBuffer,
    pubsub::{
        framed::{FramePublisher, FrameSubscriber},
        SliceBufferProvider,
    },
    state::{Inflight, PendingAck, Shared},
    Broker, TxHeader,
};

#[cfg(feature = "qos2")]
use crate::encoding::{PubComp, PubRec, PubRel};

pub(crate) struct Network<'a, N: TcpConnect> {
    network: &'a N,
    socket: Option<N::Connection<'a>>,
    packet_buf: PacketBuffer<128>,
}

impl<'a, N: TcpConnect> Network<'a, N> {
    pub fn new(network: &'a N) -> Self {
        Self {
            network,
            socket: None,
            packet_buf: PacketBuffer::new(),
        }
    }

    pub async fn connect<B: Broker>(&mut self, broker: &mut B) -> Result<(), ()> {
        let addr = broker.get_address().await.ok_or(())?;
        info!("Connecting network to {:?}", addr);
        let socket = self.network.connect(addr).await.map_err(drop)?;
        self.socket.replace(socket);

        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        self.socket.is_some()
    }

    pub async fn write_packet<P: MqttEncode>(&mut self, packet: P) -> Result<(), StateError> {
        // FIXME: Reuse packet buffer?
        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        packet
            .to_buffer(&mut encoder)
            .map_err(|_| StateError::Deserialization)?;
        self.write(encoder.bytes()).await?;
        Ok(())
    }

    pub async fn write(&mut self, buf: &[u8]) -> Result<(), StateError> {
        let Some(ref mut socket) = self.socket else {
            return Err(StateError::Io(ErrorKind::NotConnected));
        };

        trace!("Writing {} bytes to socket", buf.len());
        socket
            .write_all(buf)
            .await
            .map_err(|e| StateError::Io(e.kind()))?;
        socket.flush().await.map_err(|e| StateError::Io(e.kind()))
    }

    async fn get_received_packet(
        &mut self,
    ) -> Result<ReceivedPacket<'_, N::Connection<'a>, 128>, StateError> {
        if !self.is_connected() {
            return Err(StateError::Io(ErrorKind::NotConnected));
        };

        while !self.packet_buf.packet_available() {
            self.packet_buf
                .receive(self.socket.as_mut().unwrap())
                .await
                .map_err(|kind| {
                    self.socket.take();
                    StateError::Io(kind)
                })?;
        }

        self.packet_buf
            .received_packet(self.socket.as_mut().unwrap())
            .map_err(|_| StateError::Deserialization)
    }
}

pub struct MqttStack<'a, M: RawMutex, B, N: TcpConnect, const SUBS: usize> {
    shared: &'a Mutex<M, RefCell<Shared<SUBS>>>,
    tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
    rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, SUBS>,

    config: Config<B>,

    last_network_action: Instant,
    await_pingresp: Option<Instant>,

    clean_start: bool,

    // Network handle
    network: Network<'a, N>,
}

impl<'a, M: RawMutex, B: Broker, N: TcpConnect, const SUBS: usize> MqttStack<'a, M, B, N, SUBS> {
    pub(crate) fn new(
        network: &'a N,
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

            clean_start: true,

            last_network_action: Instant::now(),
            await_pingresp: None,

            network: Network::new(network),
        }
    }

    pub async fn run(&mut self) -> ! {
        info!("Running stack!");
        loop {
            if !self.network.is_connected() {
                self.network.connect(&mut self.config.broker).await;
                self.connect_mqtt().await;
            }

            if let Err(e) = self.select().await {
                error!("Stack error {}", e);
                // Clean state
            }
        }
    }

    async fn select(&mut self) -> Result<(), StateError> {
        let keep_alive_sleep = if let Some(instant) = self.await_pingresp {
            Timer::after(instant + self.config.keepalive_interval - Instant::now())
        } else {
            Timer::after(self.last_network_action + self.config.keepalive_interval - Instant::now())
        };

        match select3(
            self.network.get_received_packet(),
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
                debug!("Incoming packet: {:?}", packet);
                self.last_network_action = Instant::now();

                match packet {
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
                                let puback = PubAck { pid };
                                self.network.write_packet(puback).await?;
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
                                self.network.write_packet(pubrec).await?;
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
                                self.network.write_packet(pubcomp).await?;
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
                                self.network.write_packet(pubrel).await?;
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
            Either3::Second(Ok(tx_grant)) => {
                // ### TX future:
                // Based on packet QoS, add PID to state & full packet to
                // retry buffer, before writing the packet to network
                let (tx_header, packet_bytes) = TxHeader::from_bytes(tx_grant.deref());

                let shared_ref = self.shared.lock().await;
                let mut shared = shared_ref.borrow_mut();

                match tx_header.typ {
                    PacketType::Publish => {
                        if tx_header.qos != Some(QoS::AtMostOnce) {
                            debug!("Inserting {:?} into outgoing_pub", tx_header.pid.get());
                            shared
                                .outgoing_pub
                                .insert(tx_header.pid.get(), Inflight::new(packet_bytes))
                                .unwrap();
                        }
                    }
                    PacketType::Subscribe => {
                        debug!("Inserting {:?} into pending_ack", tx_header.pid.get());
                        shared
                            .pending_ack
                            .insert(PendingAck::Subscribe(tx_header.pid.get()))
                            .unwrap();
                    }
                    PacketType::Unsubscribe => {
                        debug!("Inserting {:?} into pending_ack", tx_header.pid.get());
                        shared
                            .pending_ack
                            .insert(PendingAck::Unsubscribe(tx_header.pid.get()))
                            .unwrap();
                    }
                    e => {
                        // Request has invalid header? This should never be possible!
                        // Just log an error, drop the full request packet, and continue handling next packet

                        error!("TX Packet has invalid header?! Dropping packet {:?}", e);
                        tx_grant.release();
                        return Ok(());
                    }
                }

                self.network.write(packet_bytes).await?;

                shared.wake_tx();
                self.last_network_action = Instant::now();

                tx_grant.release();
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
                self.network.write_packet(pingreq).await?;

                self.last_network_action = Instant::now();
                self.await_pingresp = Some(Instant::now());
            }
            _ => {}
        }
        Ok(())
    }

    async fn connect_mqtt(&mut self) -> Result<bool, ConnectionError> {
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
        self.network
            .write_packet(connect)
            .await
            .map_err(|e| ConnectionError::MqttState(e))?;

        self.last_network_action = Instant::now();

        // validate connack
        // TODO: ERROR types
        match self
            .network
            .get_received_packet()
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

                self.clean_start = false;

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

    // #[tokio::test]
    // async fn subscribe_publish() {
    //     let network = MockNetwork;

    //     // Create the MQTT stack
    //     let state = static_cell::make_static!(State::<NoopRawMutex, 4096, 4096, 4>::new());
    //     let config = Config::new("client_id", IpBroker::new(Ipv4Addr::LOCALHOST, 1883));
    //     let (mut stack, client) = crate::new(state, config, &network);

    //     let fut = async {
    //         // Use the MQTT client to subscribe

    //         let subscribe = Subscribe::new(&[SubscribeTopic {
    //             topic_path: "ABC",
    //             qos: QoS::AtLeastOnce,
    //         }]);

    //         let mut subscription = client.subscribe(subscribe).await.unwrap();

    //         client
    //             .publish(Publish {
    //                 dup: false,
    //                 qos: QoS::AtLeastOnce,
    //                 pid: None,
    //                 retain: false,
    //                 topic_name: "ABC",
    //                 payload: b"",
    //                 properties: crate::encoding::Properties::Slice(&[]),
    //             })
    //             .await
    //             .unwrap();

    //         while let Some(message) = subscription.next().await {
    //             match message.topic() {
    //                 "ABC" => {
    //                     client
    //                         .publish(Publish {
    //                             dup: false,
    //                             qos: QoS::AtLeastOnce,
    //                             pid: None,
    //                             retain: false,
    //                             topic_name: "ABC",
    //                             payload: b"",
    //                             properties: crate::encoding::Properties::Slice(&[]),
    //                         })
    //                         .await
    //                         .unwrap();
    //                 }
    //                 _ => {}
    //             }
    //         }
    //     };

    //     embassy_futures::select::select(stack.run(), fut).await;
    // }

    // #[tokio::test]
    // async fn multiple_subscribe() {
    //     let network = MockNetwork;

    //     // Create the MQTT stack
    //     let state = static_cell::make_static!(State::<NoopRawMutex, 4096, 4096, 4>::new());
    //     let config = Config::new("client_id", IpBroker::new(Ipv4Addr::LOCALHOST, 1883));
    //     let (mut stack, client) = crate::new(state, config, &network);

    //     // Use the MQTT client to subscribe

    //     client
    //         .publish(Publish {
    //             dup: false,
    //             qos: QoS::AtLeastOnce,
    //             pid: None,
    //             retain: false,
    //             topic_name: "ABC",
    //             payload: b"",
    //             properties: crate::encoding::Properties::Slice(&[]),
    //         })
    //         .await
    //         .unwrap();

    //     let fut_a = async {
    //         let subscribe_a = Subscribe::new(&[SubscribeTopic {
    //             topic_path: "ABC",
    //             qos: QoS::AtLeastOnce,
    //         }]);

    //         let mut subscription_a = client.subscribe(subscribe_a).await.unwrap();
    //         while let Some(message) = subscription_a.next().await {
    //             match message.topic() {
    //                 "ABC" => {
    //                     client
    //                         .publish(Publish {
    //                             dup: false,
    //                             qos: QoS::AtLeastOnce,
    //                             pid: None,
    //                             retain: false,
    //                             topic_name: "CDE",
    //                             payload: b"",
    //                             properties: crate::encoding::Properties::Slice(&[]),
    //                         })
    //                         .await
    //                         .unwrap();
    //                     break;
    //                 }
    //                 _ => {}
    //             }
    //         }
    //     };

    //     let fut_b = async {
    //         let subscribe_b = Subscribe::new(&[SubscribeTopic {
    //             topic_path: "CDE",
    //             qos: QoS::AtLeastOnce,
    //         }]);

    //         let mut subscription_b = client.subscribe(subscribe_b).await.unwrap();

    //         while let Some(message) = subscription_b.next().await {
    //             match message.topic() {
    //                 "CDE" => {
    //                     client
    //                         .publish(Publish {
    //                             dup: false,
    //                             qos: QoS::AtLeastOnce,
    //                             pid: None,
    //                             retain: false,
    //                             topic_name: "ABC",
    //                             payload: b"",
    //                             properties: crate::encoding::Properties::Slice(&[]),
    //                         })
    //                         .await
    //                         .unwrap();
    //                     break;
    //                 }
    //                 _ => {}
    //             }
    //         }
    //     };

    //     embassy_futures::select::select(stack.run(), embassy_futures::join::join(fut_a, fut_b))
    //         .await;
    // }
}
