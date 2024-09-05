use core::ops::{Deref, DerefMut};

use embassy_futures::select::{select3, Either3};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embassy_time::Duration;
use embassy_time::{Instant, Timer};
use embedded_io_async::{Error as _, ReadExactError};
use embedded_io_async::{ErrorKind, Write as _};

use crate::Property;
use crate::{
    config::Config,
    encoder::TxHeader,
    encoding::{
        received_packet::ReceivedPacket, Connect, PingReq, Protocol, PubAck, QoS, QosPid,
        StateError,
    },
    error::ConnectionError,
    packet::PacketBuffer,
    pubsub::{
        framed::{FramePublisher, FrameSubscriber},
        SliceBufferProvider,
    },
    state::{AckStatus, Shared},
    transport::Transport,
    Disconnect,
};

#[cfg(feature = "qos2")]
use crate::encoding::{PubComp, PubRec, PubRel};

pub struct MqttStack<'a, M: RawMutex, const SUBS: usize> {
    shared: &'a Mutex<M, Shared<SUBS>>,
    tx_subscriber: FrameSubscriber<'a, SliceBufferProvider<'a>, 1>,
    rx_publisher: FramePublisher<'a, SliceBufferProvider<'a>, SUBS>,

    config: Config,
    packet_buf: PacketBuffer<128>,

    last_network_action: Instant,
    await_pingresp: Option<Instant>,

    clean_start: bool,
    connect_attempts: u8,
}

impl<'a, M: RawMutex, const SUBS: usize> MqttStack<'a, M, SUBS> {
    pub(crate) fn new(
        config: Config,
        shared: &'a Mutex<M, Shared<SUBS>>,
        tx_subscriber: FrameSubscriber<'a, SliceBufferProvider<'a>, 1>,
        rx_publisher: FramePublisher<'a, SliceBufferProvider<'a>, SUBS>,
    ) -> Self {
        Self {
            shared,
            tx_subscriber,
            rx_publisher,

            config,
            packet_buf: PacketBuffer::new(),

            clean_start: true,
            connect_attempts: 0,

            last_network_action: Instant::now(),
            await_pingresp: None,
        }
    }

    pub async fn run(&mut self, transport: &mut impl Transport) {
        loop {
            if !transport.is_connected() {
                self.shared.lock().await.set_connected(None);

                let result = embassy_time::with_timeout(self.config.connect_timeout, async {
                    transport.connect().await?;

                    self.connect_mqtt(transport).await
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
                        transport.disconnect().ok();
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
                transport.disconnect().ok();
                self.await_pingresp = None;
                self.connect_attempts = 0;
                self.packet_buf.reset();
            }
        }
    }

    pub async fn disconnect(
        &mut self,
        transport: &mut impl Transport,
    ) -> Result<(), ConnectionError> {
        let disconnect = Disconnect {
            reason_code: Default::default(),
            #[cfg(feature = "mqttv5")]
            properties: crate::Properties::Slice(&[Property::SessionExpiryInterval(0)]),

            #[cfg(feature = "mqttv3")]
            _marker: core::marker::PhantomData,
        };

        debug!("Disconnecting from MQTT");
        self.packet_buf
            .write_packet(transport, disconnect)
            .await
            .map_err(ConnectionError::MqttState)?;

        transport.disconnect()?;
        self.shared.lock().await.set_connected(None);
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
                // Handle all incoming packet types by sending ack & waking
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
                        topic_name,
                        ref mut publish,
                    } => {
                        // Handle incoming `Publish` type packets by copying the full
                        // packet to any `shared.rx_publisher` buffers where topic
                        // matches their topic_filter.

                        // If the incoming topic matches any of the topicfilters in AwaitingUnsubAck
                        // ignore them, as there will be no subscribers to remove them from the queue again
                        // The topic filters will be added to AwaitingUnSubAck when drop is called on Subscription

                        let mut ignore_message = false;

                        let shared = self.shared.lock().await;
                        let iter = shared.ack_status.iter();

                        for (_, status) in iter {
                            let AckStatus::AwaitingUnsubAck(filters) = status else {
                                continue;
                            };

                            for filter in filters {
                                if filter.is_match(topic_name) {
                                    error!("Got MSG on Publish but the topic: {} matched a topic in AwaitingUnSubAck: {}", topic_name, filter.filter());
                                    ignore_message = true;
                                }
                            }
                        }

                        // Shared is dropped as we don't want to hold on to the lock while awaiting below.
                        // Not dropping shared caused
                        drop(shared);

                        if !ignore_message {
                            match self.rx_publisher.grant_async(publish.len()).await {
                                Ok(mut grant) => {
                                    publish.copy_all(grant.deref_mut()).await.map_err(
                                        |e| match e {
                                            ReadExactError::UnexpectedEof => {
                                                StateError::Io(ErrorKind::BrokenPipe)
                                            }
                                            ReadExactError::Other(i) => StateError::Io(i.kind()),
                                        },
                                    )?;

                                    // calling `commit` will wake all subscribers
                                    grant.commit(publish.len());
                                }
                                Err(_) => {
                                    error!(
                                        "Packet is larger than the storage allocated in `rx_publisher`"
                                    );
                                }
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
                        let mut shared = self.shared.lock().await;
                        debug!("Removing PID {:?} from inflight_pub", pid.get());
                        if !shared.inflight_pub.remove(&pid.get()) {
                            warn!("Unexpected Puback, PID: {:?}", pid.get());
                        }
                        // TODO: Handle collisions (See rumqttc state.rs)
                        shared.wake_tx();
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubRel { pid, .. } => {
                        let mut shared = self.shared.lock().await;
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
                        let mut shared = self.shared.lock().await;
                        match shared.inflight_pub.remove(&pid.get()) {
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

                        let mut shared = self.shared.lock().await;
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
                    ReceivedPacket::SubAck { pid, codes, .. } => {
                        let mut shared = self.shared.lock().await;
                        // Pop pid from pending_ack
                        debug!("Received suback: {:?}, {:?}", pid, codes);

                        match shared.ack_status.get_mut(&pid.get()) {
                            Some(status) if *status == AckStatus::AwaitingSubAck => {
                                *status =
                                    AckStatus::Acked(heapless::Vec::from_slice(codes).unwrap())
                            }
                            None | Some(_) => {
                                error!("Unsolicited suback packet: {:?}", pid);
                                return Err(StateError::Unsolicited(pid.get()));
                            }
                        }

                        shared.wake_tx();
                    }
                    ReceivedPacket::UnsubAck { pid, codes, .. } => {
                        let mut shared = self.shared.lock().await;

                        // Pop pid from pending_ack
                        match shared.ack_status.get_mut(&pid.get()) {
                            Some(status) if matches!(*status, AckStatus::AwaitingUnsubAck(_)) => {
                                *status =
                                    AckStatus::Acked(heapless::Vec::from_slice(codes).unwrap())
                            }
                            None | Some(_) => {
                                error!("Unsolicited suback packet: {:?}", pid);
                                return Err(StateError::Unsolicited(pid.get()));
                            }
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

                let mut shared = self.shared.lock().await;

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

                if let Some(pid) = tx_header.pid {
                    shared.outgoing_pid.remove(&pid.get());
                }

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
            properties: crate::encoding::Properties::Slice(&[Property::SessionExpiryInterval(600)]),
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
            .map_err(ConnectionError::MqttState)?;

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
                #[cfg(feature = "mqttv5")]
                properties,
            } if reason_code.success() => {
                if self.clean_start {
                    debug!("Connected! Reusing existing session: {}", session_present);
                } else {
                    debug!("Reconnected! Reusing existing session: {}", session_present);
                }

                self.clean_start = false;
                self.connect_attempts = 0;

                #[cfg(feature = "mqttv5")]
                for prop in properties.iter() {
                    match prop {
                        Ok(Property::ServerKeepAlive(keep_alive)) => {
                            self.config.keepalive_interval = self
                                .config
                                .keepalive_interval
                                .min(Duration::from_secs(keep_alive as u64));
                        }
                        Ok(Property::MaximumQoS(qos)) => {
                            self.config.max_qos = self
                                .config
                                .max_qos
                                .max(QoS::try_from(qos).unwrap_or(QoS::AtLeastOnce));
                        }
                        _ => (),
                    }
                }

                Ok(session_present)
            }
            ReceivedPacket::ConnAck {
                reason_code,
                properties,
                ..
            } => {
                error!("Connection refused! reason code: {:?}", reason_code);

                #[cfg(feature = "mqttv5")]
                for prop in properties.iter() {
                    match prop {
                        Ok(Property::ReasonString(reason)) => {
                            error!(" => {}", reason);
                        }
                        _ => (),
                    }
                }
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

        async fn connect(
            &self,
            _remote: embedded_nal_async::SocketAddr,
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
            IpBroker::new(Ipv4Addr::LOCALHOST, 1883),
        );

        // Create the MQTT stack
        static STATE: StaticCell<State<NoopRawMutex, 4096, 4096, 4>> = StaticCell::new();
        let state = STATE.init(State::<NoopRawMutex, 4096, 4096, 4>::new());
        let config = Config::new("client_id");
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
                if let "ABC" = message.topic_name() {
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
            IpBroker::new(Ipv4Addr::LOCALHOST, 1883),
        );

        // Create the MQTT stack
        static STATE: StaticCell<State<NoopRawMutex, 4096, 4096, 4>> = StaticCell::new();
        let state = STATE.init(State::<NoopRawMutex, 4096, 4096, 4>::new());
        let config = Config::new("client_id");
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
                if let "ABC" = message.topic_name() {
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
                if let "CDE" = message.topic_name() {
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
            }
        };

        embassy_futures::select::select(
            stack.run(&mut network),
            embassy_futures::join::join(fut_a, fut_b),
        )
        .await;
    }
}
