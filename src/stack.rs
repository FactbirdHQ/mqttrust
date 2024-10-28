use core::ops::{Deref as _, DerefMut};

use embassy_futures::select::{select3, Either3};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embassy_time::Duration;
use embassy_time::{Instant, Timer};
use embedded_io_async::{Error as _, ReadExactError};
use embedded_io_async::{ErrorKind, Write as _};

use crate::crate_config::MAX_SUBSCRIBERS;
use crate::decoder::MqttDecoder;

use crate::{
    config::Config,
    encoding::{
        received_packet::ReceivedPacket, Connect, PingReq, Protocol, PubAck, QoS, QosPid,
        StateError,
    },
    error::ConnectionError,
    packet::PacketBuffer,
    pubsub::{FramePublisher, FrameSubscriber, SliceBufferProvider},
    state::{AckStatus, Shared},
    transport::Transport,
    Disconnect,
};
use crate::{SubAck, UnsubAck};

#[cfg(feature = "mqttv5")]
use crate::{Properties, Property, PubAckReasonCode};

#[cfg(feature = "qos2")]
use crate::{PubComp, PubRec, PubRel};

pub struct MqttStack<'a, M: RawMutex> {
    shared: &'a Mutex<M, Shared>,
    tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
    rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, MAX_SUBSCRIBERS>,

    config: Config,
    packet_buf: PacketBuffer,

    last_network_action: Instant,
    awaiting_pingresp: bool,

    clean_start: bool,
    connect_attempts: u8,
}

impl<'a, M: RawMutex> MqttStack<'a, M> {
    pub(crate) fn new(
        config: Config,
        shared: &'a Mutex<M, Shared>,
        tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
        rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, MAX_SUBSCRIBERS>,
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
            awaiting_pingresp: false,
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
                self.awaiting_pingresp = false;
                self.connect_attempts = 0;
                self.shared.lock().await.reset();
                self.packet_buf.reset();
            }
        }
    }

    pub async fn disconnect(
        &mut self,
        transport: &mut impl Transport,
    ) -> Result<(), ConnectionError> {
        let disconnect = Disconnect::builder();
        #[cfg(feature = "mqttv5")]
        let disconnect =
            disconnect.properties(Properties::Slice(&[Property::SessionExpiryInterval(0)]));

        debug!("Disconnecting from MQTT");
        self.packet_buf
            .write_packet(transport, disconnect.build())
            .await
            .map_err(ConnectionError::MqttState)?;

        transport.disconnect()?;
        self.shared.lock().await.reset();
        Ok(())
    }

    async fn select(&mut self, transport: &mut impl Transport) -> Result<(), StateError> {
        let now = Instant::now();
        let keep_alive_sleep = if let Some(remaining_time) =
            (self.last_network_action + self.config.keepalive_interval).checked_duration_since(now)
        {
            Timer::after(remaining_time)
        } else {
            Timer::after(Duration::MIN)
        };

        match select3(
            self.tx_subscriber.read_async(),
            self.packet_buf.get_received_packet(transport),
            keep_alive_sleep,
        )
        .await
        {
            Either3::First(Ok(tx_grant)) => {
                // ### TX future:
                transport
                    .socket()?
                    .write_all(tx_grant.deref())
                    .await
                    .map_err(|e| StateError::Io(e.kind()))?;

                transport
                    .socket()?
                    .flush()
                    .await
                    .map_err(|e| StateError::Io(e.kind()))?;

                let mut shared = self.shared.lock().await;

                let mut decoder = MqttDecoder::try_new(tx_grant.deref()).unwrap();
                if let Some(pid) = decoder.read_pid() {
                    shared.outgoing_pid.remove(&pid.get());
                    trace!(
                        "Outgoing PID {} successfully transmitted! Missing tx: {:?}",
                        pid.get(),
                        shared.outgoing_pid.len()
                    );
                }

                tx_grant.release();
                shared.wake_tx();

                self.last_network_action = Instant::now();
            }
            Either3::Second(Ok(mut packet)) => {
                // ### RX future:
                //
                // Handle all incoming packet types by sending ack & waking
                // // `tx_wakers`.
                match packet {
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
                    ReceivedPacket::ConnAck(_) => {
                        // This should never happen, as this function is not
                        // used until after successfully connected to MQTT
                        // broker
                        warn!("Got unexpected connack");
                    }
                    ReceivedPacket::PartialPublish {
                        qos_pid,
                        topic_name,
                        ref mut publish,
                    } => {
                        // Handle incoming `Publish` type packets by copying the full
                        // packet to any `shared.rx_publisher` buffers where topic
                        // matches their topic_filter.

                        // If the incoming topic matches any of the topicfilters
                        // in AwaitingUnsubAck ignore them, as there will be no
                        // subscribers to remove them from the queue again The
                        // topic filters will be added to AwaitingUnSubAck when
                        // drop is called on Subscription
                        let shared = self.shared.lock().await;

                        let ignore_message = shared.ack_status.iter().any(|(_, status)| {
                            // Only check AwaitingUnsubAck status
                            if let AckStatus::AwaitingUnsubAck(Some(filters)) = status {
                                filters.iter().any(|f| f.is_match(topic_name))
                            } else {
                                false
                            }
                        });

                        // Shared is dropped as we don't want to hold on to the lock while awaiting below.
                        drop(shared);

                        if !ignore_message {
                            trace!("Adding {:?} as available for subscribers", topic_name);

                            match self.rx_publisher.grant_immediate(publish.len()) {
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
                                Err(e) => {
                                    error!(
                                        "Packet is larger than the storage allocated in `rx_publisher`. {:?}", e
                                    );
                                    return Ok(());
                                }
                            }
                        } else {
                            // TODO: Do we need to read remaining bytes of PartialPublish when we ignore the message here?¨
                            error!("Ignore message with topic {:?}", topic_name);
                        }

                        // Write `PubAck` or `PubRec` depending on received QoS
                        match qos_pid {
                            QosPid::AtMostOnce => {}
                            QosPid::AtLeastOnce(pid) => {
                                warn!("sending puback {:?}", pid);
                                let puback = PubAck {
                                    pid,
                                    #[cfg(feature = "mqttv5")]
                                    reason_code: PubAckReasonCode::Success,
                                    #[cfg(feature = "mqttv5")]
                                    properties: Properties::Slice(&[]),
                                };
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
                    ReceivedPacket::PubAck(p) => {
                        let mut shared = self.shared.lock().await;
                        debug!("Removing PID {:?} from inflight_pub", p.pid.get());
                        if !shared.inflight_pub.remove(&p.pid.get()) {
                            warn!("Unexpected Puback, PID: {:?}", p.pid.get());
                        }
                        #[cfg(feature = "mqttv5")]
                        if p.reason_code != PubAckReasonCode::Success {
                            warn!("Received PUBACK with reason code: {:?}", p.reason_code);
                        }
                        // TODO: Handle collisions (See rumqttc state.rs)
                        shared.wake_tx();
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubRel(p) => {
                        let mut shared = self.shared.lock().await;
                        match shared.incoming_pub.remove(&p.pid.get()) {
                            true => {
                                let pubcomp = PubComp { pid: p };
                                self.packet_buf.write_packet(transport, pubcomp).await?;
                                shared.wake_tx();
                            }
                            false => {
                                error!("Unsolicited pubrel packet: {:?}", p.pid);
                            }
                        }
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubRec(p) => {
                        let mut shared = self.shared.lock().await;
                        match shared.inflight_pub.remove(&p.pid.get()) {
                            Some(_) => {
                                shared.outgoing_rel.insert(p.pid.get());

                                let pubrel = PubRel { pid: p };
                                self.packet_buf.write_packet(transport, pubrel).await?;
                                shared.wake_tx();
                            }
                            None => {
                                error!("Unsolicited pubrec packet: {:?}", p.pid);
                            }
                        }
                    }
                    #[cfg(feature = "qos2")]
                    ReceivedPacket::PubComp(p) => {
                        // TODO: Handle collisions (See rumqttc state.rs)

                        let mut shared = self.shared.lock().await;
                        match shared.outgoing_rel.remove(&p.pid.get()) {
                            true => {
                                shared.wake_tx();
                            }
                            false => {
                                error!("Unsolicited pubcomp packet: {:?}", p.pid);
                            }
                        }
                    }
                    ReceivedPacket::SubAck(SubAck { pid, codes, .. }) => {
                        let mut shared = self.shared.lock().await;
                        // Pop pid from pending_ack
                        debug!("Received suback: {:?}, {:?}", pid, codes);

                        match shared.ack_status.get_mut(&pid.get()) {
                            Some(status) if *status == AckStatus::AwaitingSubAck => {
                                *status =
                                    AckStatus::Acked(heapless::Vec::from_slice(codes).unwrap());
                                shared.wake_tx();
                            }
                            None | Some(_) => {
                                error!("Unsolicited suback packet: {:?}", pid);
                            }
                        }
                    }
                    ReceivedPacket::UnsubAck(UnsubAck { pid, codes, .. }) => {
                        let mut shared = self.shared.lock().await;

                        // Pop pid from pending_ack
                        match shared.ack_status.get_mut(&pid.get()) {
                            Some(status)
                                if matches!(*status, AckStatus::AwaitingUnsubAck(None)) =>
                            {
                                *status =
                                    AckStatus::Acked(heapless::Vec::from_slice(codes).unwrap());
                                shared.wake_tx();
                            }
                            // Remove the entry in case this was part of a subscription being dropped
                            Some(status)
                                if matches!(*status, AckStatus::AwaitingUnsubAck(Some(_))) =>
                            {
                                shared.ack_status.remove(&pid.get());
                                shared.wake_tx();
                            }
                            None | Some(_) => {
                                error!("Unsolicited suback packet: {:?}", pid);
                            }
                        }
                    }
                }

                self.last_network_action = Instant::now();

                // Reset the buffer in `PacketReader`, so we are ready for the
                // next MQTT packet
                self.packet_buf.reset();
            }
            Either3::Third(_) => {
                // ### PING future:

                // Raise error if last ping didn't receive PINGRESP
                if self.awaiting_pingresp {
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
                self.awaiting_pingresp = true;
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
            properties: Properties::Slice(&[Property::SessionExpiryInterval(600)]),
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
        let result = match self
            .packet_buf
            .get_received_packet(transport)
            .await
            .map_err(|_| ConnectionError::FlushTimeout)?
        {
            ReceivedPacket::ConnAck(p) => {
                #[cfg(feature = "mqttv5")]
                for prop in p.properties.iter() {
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
                        Ok(Property::ReasonString(reason)) => {
                            error!(" => {}", reason);
                        }
                        _ => (),
                    }
                }

                if p.reason_code.success() {
                    if self.clean_start {
                        debug!("Connected! Reusing existing session: {}", p.session_present);
                    } else {
                        debug!(
                            "Reconnected! Reusing existing session: {}",
                            p.session_present
                        );
                    }

                    self.clean_start = false;
                    self.connect_attempts = 0;

                    Ok(p.session_present)
                } else {
                    error!("Connection refused! reason code: {:?}", p.reason_code);
                    Err(ConnectionError::ConnectionRefused)
                }
            }
            _ => Err(ConnectionError::NotConnAck),
        };

        // Reset the buffer in `PacketReader`, so we are ready for the next MQTT packet
        self.packet_buf.reset();

        result
    }
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embedded_nal_async::{Ipv4Addr, TcpConnect};
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
            IpBroker::new(Ipv4Addr::LOCALHOST, 1883),
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
