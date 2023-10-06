use core::{
    cell::RefCell,
    future::poll_fn,
    ops::{Deref, DerefMut},
    task::Poll,
};

use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};

use crate::{
    encoding::v4::{self, Packet, Pid, Publish, Subscribe, SubscribeTopic},
    error::Error,
    packet::SerializedPacket,
    pubsub::{
        framed::{FrameGrantR, FramePublisher, FrameSubscriber},
        PubSubChannel, SliceBufferProvider,
    },
    state::Shared,
    SUBS,
};

pub struct MqttClient<'a, M: RawMutex> {
    tx_publisher: Mutex<M, RefCell<FramePublisher<'a, M, SliceBufferProvider<'a>, 1>>>,
    shared: &'a Mutex<M, RefCell<Shared>>,
    rx_pubsub: &'a PubSubChannel<M, SliceBufferProvider<'a>, SUBS>,
}

impl<'a, M: RawMutex> MqttClient<'a, M> {
    pub(crate) fn new(
        tx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, 1>,
        shared: &'a Mutex<M, RefCell<Shared>>,
        rx_pubsub: &'a PubSubChannel<M, SliceBufferProvider<'a>, SUBS>,
    ) -> Self {
        Self {
            tx_publisher: Mutex::new(RefCell::new(tx_publisher)),
            shared,
            rx_pubsub,
        }
    }

    async fn send(&self, packet: &Packet<'_>) -> Result<Pid, Error> {
        let pid = self.shared.lock().await.borrow_mut().next_pid();

        // Push the serialized packet into `tx_publisher`
        let tx_prod_ref = self.tx_publisher.lock().await;
        let mut tx_prod = tx_prod_ref.borrow_mut();

        let max_size = packet.len();
        let mut grant = tx_prod.grant(max_size).map_err(|_| Error::StateMismatch)?;

        let actual_len =
            v4::encode_slice(&packet, grant.deref_mut()).map_err(|_| Error::MalformedPacket)?;
        grant.commit(actual_len);

        // Wait until `pid` has been ack'd (known by it being removed from the state)
        // FIXME: How to know if it's an ACK or a NACK?
        poll_fn(|cx| {
            if let Ok(shared) = self.shared.try_lock() {
                let mut state = shared.borrow_mut();

                // Check if `pid` has been acked by the broker
                if state.outgoing_pub.get(&pid.get()).is_none() {
                    return Poll::Ready(());
                }

                state.register_tx_waker(cx);
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;

        Ok(pid)
    }

    pub async fn publish(&self, packet: impl Into<Publish<'_>>) -> Result<(), Error> {
        self.send(&Packet::Publish(packet.into())).await?;
        Ok(())
    }

    pub async fn subscribe(
        &self,
        packet: impl Into<Subscribe<'_>>,
    ) -> Result<Subscription<'a, M>, Error> {
        let subscriber = self
            .rx_pubsub
            .framed_subscriber()
            .map_err(|_| Error::StateMismatch)?;

        let subscribe = packet.into();
        let topic_filter = TopicFilter::new(subscribe.topics());

        self.send(&Packet::Subscribe(subscribe)).await?;

        Ok(Subscription {
            subscriber,
            topic_filter,
        })
    }
}

// FIXME: Build a topic filter we can evaluate incoming message against
pub struct TopicFilter;

impl TopicFilter {
    pub fn new<'a>(topics: impl Iterator<Item = SubscribeTopic<'a>>) -> Self {
        Self
    }

    pub fn matches(&self, topic: &str) -> bool {
        false
    }
}

pub struct Subscription<'a, M: RawMutex> {
    topic_filter: TopicFilter,
    subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, SUBS>,
}

impl<'a, M: RawMutex> futures::Stream for Subscription<'a, M> {
    type Item = Message<'a, M>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if let Some(mut msg) = self.subscriber.read() {
            let packet = SerializedPacket(msg.deref_mut());
            if let Some(topic) = packet.topic() {
                if self.topic_filter.matches(topic) {
                    // This message is indeed for us! Make it auto release on drop
                    msg.auto_release(true);
                    return Poll::Ready(Some(Message(msg)));
                }
            }
        }

        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

// FIXME: Change Message type
pub struct Message<'a, M: RawMutex>(FrameGrantR<'a, M, SliceBufferProvider<'a>, SUBS>);

impl<'a, M: RawMutex> Message<'a, M> {
    pub fn topic(&self) -> &str {
        core::str::from_utf8(&self.0.deref()[..10]).unwrap()
    }

    pub fn payload(&self) -> &[u8] {
        &self.0.deref()[10..]
    }
}
