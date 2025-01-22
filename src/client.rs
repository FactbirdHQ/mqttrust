use core::{future::poll_fn, ops::DerefMut, task::Poll};

use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embassy_time::{with_timeout, Duration};

use crate::{
    crate_config::{MAX_CLIENT_ID_LEN, MAX_SUBSCRIBERS},
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        Pid, Publish, QoS, Subscribe,
    },
    error::Error,
    message::Message,
    pubsub::{FramePublisher, FrameSubscriber, PubSubChannel, SliceBufferProvider},
    state::{AckStatus, Shared},
    topic_filter::TopicFilter,
    OnDrop, PacketType, Properties, ToPayload, Unsubscribe,
};

/// Represents an MQTT client that can publish and subscribe to topics.
pub struct MqttClient<'a, M: RawMutex> {
    tx_publisher: Mutex<M, FramePublisher<'a, M, SliceBufferProvider<'a>, 1>>,
    shared: &'a Mutex<M, Shared>,
    rx_pubsub: &'a PubSubChannel<M, SliceBufferProvider<'a>, { MAX_SUBSCRIBERS }>,
    client_id: heapless::String<MAX_CLIENT_ID_LEN>,
}

impl<'a, M: RawMutex> MqttClient<'a, M> {
    /// Creates a new `MqttClient`.
    ///
    /// # Parameters
    ///
    /// - `tx_publisher`: The publisher for transmitting MQTT frames.
    /// - `shared`: A shared state for managing MQTT connection and state.
    /// - `rx_pubsub`: The pub-sub channel for receiving MQTT frames.
    /// - `client_id`: The client ID for the MQTT client.
    ///
    /// # Returns
    ///
    /// A new instance of `MqttClient`.
    pub(crate) fn new(
        tx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, 1>,
        shared: &'a Mutex<M, Shared>,
        rx_pubsub: &'a PubSubChannel<M, SliceBufferProvider<'a>, { MAX_SUBSCRIBERS }>,
        client_id: heapless::String<MAX_CLIENT_ID_LEN>,
    ) -> Self {
        Self {
            tx_publisher: Mutex::new(tx_publisher),
            shared,
            rx_pubsub,
            client_id,
        }
    }

    /// Returns the client ID of the MQTT client.
    pub fn client_id(&self) -> &str {
        self.client_id.as_str()
    }

    /// Checks if the client is connected to the MQTT broker.
    ///
    /// # Returns
    ///
    /// `true` if the client is connected, otherwise `false`.
    pub async fn is_connected(&self) -> bool {
        self.shared.lock().await.connected.is_some()
    }

    /// Waits until the client is connected to the MQTT broker.
    pub async fn wait_connected(&self) {
        if self.is_connected().await {
            return;
        }

        poll_fn(|cx| {
            if let Ok(mut shared) = self.shared.try_lock() {
                if shared.connected.is_some() {
                    return Poll::Ready(());
                }

                shared.connection_waker.register(cx.waker());
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;
    }

    /// Cleans the session by waiting for the connection state to change.
    async fn clean_session(&self) {
        let mut prev_state = self.shared.lock().await.connected;

        poll_fn(|cx| {
            if let Ok(mut shared) = self.shared.try_lock() {
                if let (None | Some(true), Some(false)) = (prev_state, shared.connected) {
                    return Poll::Ready(());
                }
                prev_state = shared.connected;
                shared.connection_waker.register(cx.waker());
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;
    }

    /// Enqueues an MQTT packet into the transmit queue, to be sent by the [`MqttStack`].
    ///
    /// # Parameters
    ///
    /// - `packet`: The MQTT packet to send.
    ///
    /// # Returns
    ///
    /// The packet identifier (PID) if the packet was sent successfully, otherwise an error.
    async fn send<P: MqttEncode>(&self, packet: &mut P) -> Result<Pid, Error> {
        let mut shared = self.shared.lock().await;

        // Check if we can send, based on `MAX_INFLIGHT` & packet.qos()
        match packet.get_qos() {
            Some(QoS::AtMostOnce) => {}
            Some(_) if shared.inflight_pub.len() == shared.inflight_pub.capacity() => {
                return Err(Error::MaxInflight)
            }
            _ => {}
        }

        let pid = shared.next_pid();
        let mut tx_prod = self.tx_publisher.lock().await;

        packet.set_pid(pid);

        let mut grant = tx_prod
            .grant_async(packet.max_packet_size())
            .await
            .map_err(|_| Error::Overflow)?;

        let mut encoder = MqttEncoder::new(grant.deref_mut());
        packet.to_buffer(&mut encoder).ok();
        let used = encoder.used_size();

        shared
            .outgoing_pid
            .insert(pid.get())
            .map_err(|_| Error::MaxInflight)?;

        match P::PACKET_TYPE {
            PacketType::Publish => {
                if packet.get_qos() != Some(QoS::AtMostOnce) {
                    trace!("[Publish] Inserting {:?} into inflight_pub", pid);
                    // # Safety: Capacity checked above by returning `Error::MaxInflight` if full
                    unwrap!(shared.inflight_pub.insert(pid.get()));
                }
            }
            PacketType::Subscribe => {
                trace!("[Subscribe] Inserting {:?} into pending_ack", pid);
                shared
                    .ack_status
                    .insert(pid.get(), AckStatus::AwaitingSubAck)
                    .map_err(|_| Error::MaxInflight)?;
            }
            PacketType::Unsubscribe => {
                trace!("[Unsubscribe] Inserting {:?} into pending_ack", pid);

                shared
                    .ack_status
                    .insert(pid.into(), AckStatus::AwaitingUnsubAck(true))
                    .map_err(|_| Error::MaxInflight)?;
            }
            _ => {}
        }

        // Wait with committing the packet until we have added the PID to
        // state successfully
        grant.commit(used);

        Ok(pid)
    }

    /// Waits until the packet with the given PID has been transmitted.
    ///
    /// # Parameters
    ///
    /// - `pid`: The packet identifier to wait for.
    async fn wait_tx(&self, pid: &Pid) {
        poll_fn(|cx| {
            if let Ok(mut shared) = self.shared.try_lock() {
                if shared.outgoing_pid.contains(&pid.get()) {
                    shared.register_tx_waker(cx);
                    return Poll::Pending;
                }

                return Poll::Ready(());
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await
    }

    /// Publishes a message to the broker.
    ///
    /// If `QoS` is set to `QoS1` or `QoS2`, this function will wait until the
    /// corresponding `PubAck` message has been received.
    ///
    /// # Parameters
    ///
    /// - `packet`: The publish packet containing the message to be sent.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the message was published successfully, otherwise an error.
    pub async fn publish<P: ToPayload>(
        &self,
        packet: impl Into<Publish<'_, P>>,
    ) -> Result<(), Error> {
        let mut pub_pkg: Publish<'_, P> = packet.into();

        const MAX_ATTEMPTS: u8 = 3;

        for attempt in 1..=MAX_ATTEMPTS {
            self.wait_connected().await;

            if attempt > 1 {
                debug!(
                    "[Publish] Attempt: {}. Sending to {:?}",
                    attempt - 1,
                    pub_pkg.topic_name
                );
            } else {
                debug!("[Publish] Sending to {:?}", pub_pkg.topic_name);
            }

            let publish_fut = async {
                let pid = self.send(&mut pub_pkg).await?;
                self.wait_publish_tx(pid, pub_pkg.qos).await
            };

            match embassy_time::with_timeout(
                embassy_time::Duration::from_secs(5 * attempt as u64),
                publish_fut,
            )
            .await
            {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(e)) if attempt == MAX_ATTEMPTS => return Err(e),
                Err(_) if attempt == MAX_ATTEMPTS => return Err(Error::Timeout),
                _ => {
                    continue;
                }
            }
        }

        Err(Error::Timeout)
    }

    /// Subscribes to one or more topics.
    ///
    /// # Parameters
    ///
    /// - `packet`: The subscribe packet containing the topics to subscribe to.
    ///
    /// # Returns
    ///
    /// A `Subscription` object that can be used to receive messages on the subscribed topics.
    pub async fn subscribe<'b, const MAX_TOPICS: usize>(
        &'b self,
        packet: impl Into<Subscribe<'_>>,
    ) -> Result<Subscription<'a, 'b, M, MAX_TOPICS>, Error> {
        const { core::assert!(MAX_TOPICS <= crate::crate_config::MAX_SUB_TOPICS_PER_MSG) };

        let subscriber = self
            .rx_pubsub
            .subscriber()
            .map_err(|_| Error::StateMismatch)?;

        let mut subscribe: Subscribe = packet.into();

        const MAX_ATTEMPTS: u8 = 3;

        for attempt in 1..=MAX_ATTEMPTS {
            self.wait_connected().await;

            // if attempt > 1 {
            //     debug!(
            //         "[Subscribe] Attempt: {}. Subscribing to {:?}",
            //         attempt - 1,
            //         topic_filters
            //     );
            // } else {
            //     debug!("[Subscribe] Subscribing to {:?}", topic_filters);
            // }

            let subscribe_fut = async {
                let pid = self.send(&mut subscribe).await?;
                self.wait_subscribe_tx(pid).await
            };

            match with_timeout(Duration::from_secs(3 * attempt as u64), subscribe_fut).await {
                Ok(Ok(())) => break,
                Ok(Err(e)) if attempt == MAX_ATTEMPTS => return Err(e),
                Err(_) if attempt == MAX_ATTEMPTS => return Err(Error::Timeout),
                _ => continue,
            }
        }

        let topic_filters = subscribe
            .topics
            .iter()
            .map(|sub| TopicFilter::new(sub.topic_path))
            .collect::<Result<_, _>>()?;

        Ok(Subscription {
            subscriber,
            topic_filters,
            client: self,
        })
    }

    async fn wait_publish_tx(&self, pid: Pid, qos: QoS) -> Result<(), Error> {
        let should_wait_ack = !matches!(qos, QoS::AtMostOnce);
        // Cleanup `inflight_pub` & `outgoing_pid` state in case of cancelled futures
        let drop = OnDrop::new(|| {
            if let Ok(mut shared) = self.shared.try_lock() {
                if shared.inflight_pub.remove(&pid.get()) || shared.outgoing_pid.remove(&pid.get())
                {
                    trace!(
                        "[DROP] Removed {:?} from either inflight_pub or outgoing_pid!",
                        pid
                    );
                }
            }
        });

        self.wait_tx(&pid).await;

        trace!("[Publish] Sent pid {:?}", pid);

        if should_wait_ack {
            // Wait until `pid` has been ack'd (known by it being removed from the state)
            poll_fn(|cx| {
                if let Ok(mut shared) = self.shared.try_lock() {
                    // Check if `pid` has been acked by the broker
                    if !shared.inflight_pub.contains(&pid.get()) {
                        trace!("publish ack {:?}", pid.get());
                        return Poll::Ready(());
                    }

                    shared.register_tx_waker(cx);
                    return Poll::Pending;
                }
                cx.waker().wake_by_ref();
                Poll::Pending
            })
            .await;
        }

        drop.defuse();

        trace!("[Publish] Success pid {:?}", pid);

        Ok(())
    }

    async fn wait_subscribe_tx(&self, pid: Pid) -> Result<(), Error> {
        // Cleanup `ack_status` & `outgoing_pid` state in case of cancelled futures
        let drop = OnDrop::new(|| {
            if let Ok(mut shared) = self.shared.try_lock() {
                trace!("[DROP] Removing {:?} from ack_status & outgoing_pid!", pid);
                let _ = shared.ack_status.remove(&pid.get());
                let _ = shared.outgoing_pid.remove(&pid.get());
            }
        });

        self.wait_tx(&pid).await;

        trace!("[Subscribe] Sent pid {:?}", pid);

        // Wait until `pid` has been ack'd (known by it being removed from the state)
        let status = poll_fn(|cx| {
            if let Ok(mut shared) = self.shared.try_lock() {
                // Check if `pid` has been acked by the broker
                if let Some(&AckStatus::Acked(_)) = shared.ack_status.get(&pid.get()) {
                    match shared.ack_status.remove(&pid.get()) {
                        Some(AckStatus::Acked(codes)) => return Poll::Ready(codes),
                        // # Safety: Should be checked above!
                        _ => panic!(),
                    }
                }

                shared.register_tx_waker(cx);
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;

        debug!("[Subscribe] Status codes: {:?}", status);
        if status.iter().any(|&c| c >= 0x80) {
            return Err(Error::BadTopicFilter);
        }

        drop.defuse();

        trace!("[Subscribe] Success pid {:?}", pid);

        Ok(())
    }
}

/// Represents a subscription to one or more MQTT topics.
pub struct Subscription<'a, 'b, M: RawMutex, const MAX_TOPICS: usize> {
    topic_filters: heapless::Vec<TopicFilter, MAX_TOPICS>,
    subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, MAX_SUBSCRIBERS>,
    client: &'b MqttClient<'a, M>,
}

impl<'a, 'b, M: RawMutex, const MAX_TOPICS: usize> Subscription<'a, 'b, M, MAX_TOPICS> {
    /// Waits for the next message on the subscribed topics.
    ///
    /// # Returns
    ///
    /// The next message if available, otherwise `None`.
    pub async fn next_message(&mut self) -> Option<Message<'a, M, SliceBufferProvider<'a>>> {
        loop {
            // FIXME: Handle unsubscribed from broker by returning `None`
            let Subscription {
                subscriber,
                topic_filters,
                client,
            } = self;

            match select(subscriber.read_async(), client.clean_session()).await {
                Either::First(Ok(grant)) => {
                    if let Some(mut message) = Message::try_new(grant) {
                        message.auto_release();
                        if topic_filters
                            .iter()
                            .any(|f| f.is_match(message.topic_name()))
                        {
                            return Some(message);
                        }
                    }
                }
                Either::Second(_) => return None,
                _ => {}
            }
        }
    }

    /// Unsubscribes from the topics.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the unsubscription was successful, otherwise an error.
    pub async fn unsubscribe(mut self) -> Result<(), Error> {
        let topics = self
            .topic_filters
            .iter()
            .map(|f| f.filter())
            .collect::<heapless::Vec<_, MAX_TOPICS>>();

        let mut packet = Unsubscribe {
            pid: None,
            #[cfg(feature = "mqttv5")]
            properties: Properties::Slice(&[]),
            topics: topics.as_slice(),
        };

        let pid = self.client.send(&mut packet).await?;

        self.client.wait_tx(&pid).await;

        // Wait until `pid` has been ack'd (known by it being removed from the state)
        let status = poll_fn(|cx| {
            if let Ok(mut shared) = self.client.shared.try_lock() {
                // Check if `pid` has been acked by the broker
                if let Some(&AckStatus::Acked(_)) = shared.ack_status.get(&pid.get()) {
                    match shared.ack_status.remove(&pid.get()) {
                        Some(AckStatus::Acked(codes)) => return Poll::Ready(codes),
                        // # Safety: Should be checked above!
                        _ => panic!(),
                    }
                }

                shared.register_tx_waker(cx);
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;

        debug!("[Unsubscribe] Status codes: {:?}", status);
        if status.iter().any(|&c| c >= 0x80) {
            return Err(Error::BadTopicFilter);
        }

        drop(topics);
        self.topic_filters.clear();

        Ok(())
    }
}

impl<'a, 'b, M: RawMutex, const MAX_TOPICS: usize> futures_util::Stream
    for Subscription<'a, 'b, M, MAX_TOPICS>
{
    type Item = Message<'a, M, SliceBufferProvider<'a>>;

    /// Polls for the next message on the subscribed topics.
    ///
    /// # Returns
    ///
    /// The next message if available, otherwise `Poll::Pending`.
    fn poll_next(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // FIXME: Handle unsubscribed from broker by returning `Poll::Ready(None)`
        let Subscription {
            subscriber,
            topic_filters,
            ..
        } = self.get_mut();

        match subscriber.read_with_context(Some(cx)) {
            Ok(mut grant) => {
                grant.auto_release(true);
                if let Some(message) = Message::try_new(grant) {
                    if topic_filters
                        .iter()
                        .any(|f| f.is_match(message.topic_name()))
                    {
                        return Poll::Ready(Some(message));
                    }
                }

                cx.waker().wake_by_ref();
                Poll::Pending
            }
            _ => Poll::Pending,
        }
    }
}
impl<'a, 'b, M: RawMutex, const MAX_TOPICS: usize> Drop for Subscription<'a, 'b, M, MAX_TOPICS> {
    /// Unsubscribes from the topics when the subscription is dropped.
    fn drop(&mut self) {
        if self.topic_filters.is_empty() {
            return;
        }

        let pid = if let Ok(mut shared) = self.client.shared.try_lock() {
            shared.next_pid()
        } else {
            Pid::new()
        };

        if let Ok(mut tx_prod) = self.client.tx_publisher.try_lock() {
            let topics = self
                .topic_filters
                .iter()
                .map(|f| f.filter())
                .collect::<heapless::Vec<_, MAX_TOPICS>>();

            let packet = Unsubscribe {
                pid: Some(pid),
                #[cfg(feature = "mqttv5")]
                properties: Properties::Slice(&[]),
                topics: topics.as_slice(),
            };

            if let Ok(mut grant) = tx_prod.grant(packet.max_packet_size()) {
                if let Ok(mut shared) = self.client.shared.try_lock() {
                    trace!("[Unsubscribe] Inserting {:?} into pending_ack", pid);

                    shared
                        .ack_status
                        .insert(pid.into(), AckStatus::AwaitingUnsubAck(false))
                        .unwrap();
                } else {
                    error!("Could not lock client shared to insert AwaitingUnSubAck");
                }

                let mut encoder = MqttEncoder::new(grant.deref_mut());
                packet.to_buffer(&mut encoder).ok();
                let used = encoder.used_size();
                grant.commit(used);
            }
        }
    }
}
