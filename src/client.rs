use core::{future::poll_fn, mem::MaybeUninit, ops::DerefMut, task::Poll};

use bitmaps::{Bits, BitsImpl};
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};

use crate::{
    crate_config::MAX_CLIENT_ID_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        Pid, Publish, QoS, Subscribe,
    },
    error::Error,
    pubsub::{FramePublisher, FrameSubscriber, Message, PubSubChannel, SliceBufferProvider},
    state::{AckStatus, Shared},
    topic_filter::TopicFilter,
    PacketType, Properties, ToPayload, Unsubscribe,
};

pub struct MqttClient<'a, M: RawMutex, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
{
    tx_publisher: Mutex<M, FramePublisher<'a, SliceBufferProvider<'a>, 1>>,
    shared: &'a Mutex<M, Shared<SUBS>>,
    rx_pubsub: &'a PubSubChannel<SliceBufferProvider<'a>, SUBS>,
    client_id: heapless::String<MAX_CLIENT_ID_LEN>,
}

impl<'a, M: RawMutex, const SUBS: usize> MqttClient<'a, M, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    pub(crate) fn new(
        tx_publisher: FramePublisher<'a, SliceBufferProvider<'a>, 1>,
        shared: &'a Mutex<M, Shared<SUBS>>,
        rx_pubsub: &'a PubSubChannel<SliceBufferProvider<'a>, SUBS>,
        client_id: heapless::String<MAX_CLIENT_ID_LEN>,
    ) -> Self {
        Self {
            tx_publisher: Mutex::new(tx_publisher),
            shared,
            rx_pubsub,
            client_id,
        }
    }

    pub fn client_id(&self) -> &str {
        self.client_id.as_str()
    }

    pub async fn is_connected(&self) -> bool {
        self.shared.lock().await.connected.is_some()
    }

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
                    debug!("[Publish] Inserting {:?} into inflight_pub", pid);
                    // # Safety: Capacity checked above by returning `Error::MaxInflight` if full
                    unwrap!(shared.inflight_pub.insert(pid.get()));
                }
            }
            PacketType::Subscribe => {
                debug!("[Subscribe] Inserting {:?} into pending_ack", pid);
                shared
                    .ack_status
                    .insert(pid.get(), AckStatus::AwaitingSubAck)
                    .map_err(|_| Error::MaxInflight)?;
            }
            _ => {}
        }

        // Wait with committing the packet until we have added the PID to
        // state successfully
        grant.commit(used);

        Ok(pid)
    }

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

    async fn publish_inner<P: ToPayload>(&self, pub_pkg: &mut Publish<'_, P>) -> Result<(), Error> {
        let should_wait_ack = !matches!(pub_pkg.qos, QoS::AtMostOnce);

        let pid = self.send(pub_pkg).await?;

        // Cleanup `inflight_pub` & `outgoing_pid` state in case of cancelled futures
        let drop = OnDrop::new(|| {
            if let Ok(mut shared) = self.shared.try_lock() {
                if shared.inflight_pub.remove(&pid.get()) || shared.outgoing_pid.remove(&pid.get())
                {
                    warn!(
                        "[DROP] Removed {:?} from either inflight_pub or outgoing_pid!",
                        pid
                    );
                }
            }
        });

        self.wait_tx(&pid).await;

        debug!("[Publish] Sent pid {:?}", pid);

        if should_wait_ack {
            // Wait until `pid` has been ack'd (known by it being removed from the state)
            poll_fn(|cx| {
                if let Ok(mut shared) = self.shared.try_lock() {
                    // Check if `pid` has been acked by the broker
                    if !shared.inflight_pub.contains(&pid.get()) {
                        debug!("publish ack {:?}", pid.get());
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

        debug!("[Publish] Success pid {:?}", pid);

        Ok(())
    }

    async fn subscribe_inner(&self, sub_pkg: &mut Subscribe<'_>) -> Result<(), Error> {
        let pid = self.send(sub_pkg).await?;

        // Cleanup `ack_status` & `outgoing_pid` state in case of cancelled futures
        let drop = OnDrop::new(|| {
            if let Ok(mut shared) = self.shared.try_lock() {
                warn!("[DROP] Removing {:?} from ack_status & outgoing_pid!", pid);
                let _ = shared.ack_status.remove(&pid.get());
                let _ = shared.outgoing_pid.remove(&pid.get());
            }
        });

        self.wait_tx(&pid).await;

        debug!("[Subscribe] Sent pid {:?}", pid);

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

        debug!("[Subscribe] Success pid {:?}", pid);

        Ok(())
    }

    /// Publish a message to the broker.
    ///
    /// If `QoS` is set to `QoS1` or `QoS2`, this function will wait until the
    /// corresponding `PubAck` message has been received.
    pub async fn publish<P: ToPayload>(
        &self,
        packet: impl Into<Publish<'_, P>>,
    ) -> Result<(), Error> {
        let mut pub_pkg: Publish<'_, P> = packet.into();

        self.publish_inner(&mut pub_pkg).await

        // const MAX_ATTEMPTS: u8 = 3;

        // for attempt in 1..=MAX_ATTEMPTS {
        //     self.wait_connected().await;

        //     if attempt > 1 {
        //         debug!(
        //             "[Publish] Attempt: {}. Sending to {:?}",
        //             attempt - 1,
        //             pub_pkg.topic_name
        //         );
        //     } else {
        //         debug!("[Publish] Sending to {:?}", pub_pkg.topic_name);
        //     }

        //     match with_timeout(
        //         Duration::from_secs(5 * attempt as u64),
        //         self.publish_inner(&mut pub_pkg),
        //     )
        //     .await
        //     {
        //         Ok(Ok(())) => return Ok(()),
        //         Ok(Err(e)) if attempt == MAX_ATTEMPTS => return Err(e),
        //         _ => {
        //             continue;
        //         }
        //     }
        // }

        // Err(Error::Timeout)
    }

    pub async fn subscribe<'b, const MAX_TOPICS: usize>(
        &'b self,
        packet: impl Into<Subscribe<'_>>,
    ) -> Result<Subscription<'a, 'b, M, SUBS, MAX_TOPICS>, Error> {
        const { core::assert!(MAX_TOPICS <= crate::crate_config::MAX_SUB_TOPICS_PER_MSG) };

        let subscriber = self
            .rx_pubsub
            .subscriber()
            .map_err(|_| Error::StateMismatch)?;

        let mut subscribe: Subscribe = packet.into();
        let topic_filters = subscribe
            .topics
            .iter()
            .map(|sub| TopicFilter::new(sub.topic_path))
            .collect::<Result<_, _>>()?;

        self.subscribe_inner(&mut subscribe).await?;

        Ok(Subscription {
            subscriber,
            topic_filters,
            client: self,
        })

        // const MAX_ATTEMPTS: u8 = 3;

        // for attempt in 1..=MAX_ATTEMPTS {
        //     self.wait_connected().await;

        //     if attempt > 1 {
        //         debug!(
        //             "[Subscribe] Attempt: {}. Subscribing to {:?}",
        //             attempt - 1,
        //             topic_filters
        //         );
        //     } else {
        //         debug!("[Subscribe] Subscribing to {:?}", topic_filters);
        //     }

        //     match with_timeout(
        //         Duration::from_secs(3 * attempt as u64),
        //         self.subscribe_inner(&mut subscribe),
        //     )
        //     .await
        //     {
        //         Ok(Ok(())) => {
        //             return Ok(Subscription {
        //                 subscriber,
        //                 topic_filters,
        //                 client: self,
        //             })
        //         }
        //         Ok(Err(e)) if attempt == MAX_ATTEMPTS => return Err(e),
        //         _ => {
        //             continue;
        //         }
        //     }
        // }

        // Err(Error::Timeout)
    }
}

struct OnDrop<F: FnOnce()> {
    f: MaybeUninit<F>,
}

impl<F: FnOnce()> OnDrop<F> {
    pub fn new(f: F) -> Self {
        Self {
            f: MaybeUninit::new(f),
        }
    }

    pub fn defuse(self) {
        core::mem::forget(self)
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        unsafe { self.f.as_ptr().read()() }
    }
}

pub struct Subscription<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
{
    topic_filters: heapless::Vec<TopicFilter, MAX_TOPICS>,
    subscriber: FrameSubscriber<'a, SliceBufferProvider<'a>, SUBS>,
    client: &'b MqttClient<'a, M, SUBS>,
}

impl<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize>
    Subscription<'a, 'b, M, SUBS, MAX_TOPICS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    pub async fn next_message(&mut self) -> Option<Message<'a, SliceBufferProvider<'a>, SUBS>> {
        // FIXME: Handle unsubscribed from broker by returning `None`
        let Subscription {
            subscriber,
            topic_filters,
            client,
        } = self;

        match select(
            subscriber.read_message_async(topic_filters.as_slice()),
            client.clean_session(),
        )
        .await
        {
            Either::First(msg) => msg.ok(),
            Either::Second(_) => None,
        }
    }
}

impl<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize> futures_util::Stream
    for Subscription<'a, 'b, M, SUBS, MAX_TOPICS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    type Item = Message<'a, SliceBufferProvider<'a>, SUBS>;

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

        match subscriber.read_message_with_context(topic_filters.as_slice(), Some(cx)) {
            Ok(message) => Poll::Ready(Some(message)),
            Err(_) => Poll::Pending,
        }
    }
}

impl<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize> Drop
    for Subscription<'a, 'b, M, SUBS, MAX_TOPICS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    fn drop(&mut self) {
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

            if let Ok(mut shared) = self.client.shared.try_lock() {
                debug!("[Unsubscribe] Inserting {:?} into pending_ack", pid);

                shared
                    .ack_status
                    .insert(
                        pid.into(),
                        AckStatus::AwaitingUnsubAck(
                            // # Safety: Checked before subscribing (Assert: MAX_TOPICS <= crate::crate_config::MAX_SUB_TOPICS_PER_MSG)
                            unwrap!(heapless::Vec::try_from(self.topic_filters.as_slice())),
                        ),
                    )
                    .unwrap();
            } else {
                error!("Could not lock client shared to insert AwaitingUnSubAck");
            }

            let packet = Unsubscribe {
                pid: Some(pid),
                #[cfg(feature = "mqttv5")]
                properties: Properties::Slice(&[]),
                topics: topics.as_slice(),
            };

            if let Ok(mut grant) = tx_prod.grant(packet.max_packet_size()) {
                let mut encoder = MqttEncoder::new(grant.deref_mut());
                packet.to_buffer(&mut encoder).ok();
                let used = encoder.used_size();
                grant.commit(used);
            }
        }
    }
}
