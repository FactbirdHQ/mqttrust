use core::{
    cell::RefCell,
    future::poll_fn,
    ops::{Deref, DerefMut, Range},
    str::FromStr,
    task::Poll,
};

use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embassy_time::{with_timeout, Duration, TimeoutError};
use itertools::EitherOrBoth;
use itertools::Itertools;

use crate::{
    encoding::{
        decoder::{FixedHeader, MqttDecoder},
        encoder::{MqttEncode, MqttEncoder},
        PacketType, Pid, Publish, QoS, QosPid, Subscribe,
    },
    error::Error,
    pubsub::{
        framed::{FrameGrantR, FramePublisher, FrameSubscriber},
        PubSubChannel, SliceBufferProvider,
    },
    state::{PendingAck, Shared},
    Properties, TxHeader, MAX_TOPIC_LEN,
};

pub struct MqttClient<'a, M: RawMutex, const SUBS: usize> {
    tx_publisher: Mutex<M, RefCell<FramePublisher<'a, M, SliceBufferProvider<'a>, 1>>>,
    shared: &'a Mutex<M, RefCell<Shared<SUBS>>>,
    rx_pubsub: &'a PubSubChannel<M, SliceBufferProvider<'a>, SUBS>,
}

impl<'a, M: RawMutex, const SUBS: usize> MqttClient<'a, M, SUBS> {
    pub(crate) fn new(
        tx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, 1>,
        shared: &'a Mutex<M, RefCell<Shared<SUBS>>>,
        rx_pubsub: &'a PubSubChannel<M, SliceBufferProvider<'a>, SUBS>,
    ) -> Self {
        Self {
            tx_publisher: Mutex::new(RefCell::new(tx_publisher)),
            shared,
            rx_pubsub,
        }
    }

    async fn send<P: MqttEncode>(&self, mut packet: P) -> Result<Pid, Error> {
        let pid = {
            let state_ref = self.shared.lock().await;
            let mut shared = state_ref.borrow_mut();

            // Check if we can send, based on `MAX_INFLIGHT` & packet.qos()
            match packet.get_qos() {
                Some(qos) if qos == QoS::AtMostOnce => {}
                Some(_) if shared.outgoing_pub.len() == shared.outgoing_pub.capacity() => {
                    return Err(Error::MaxInflight)
                }
                _ => {}
            }

            let pid = shared.next_pid();
            let tx_prod_ref = self.tx_publisher.lock().await;
            let mut tx_prod = tx_prod_ref.borrow_mut();

            packet.set_pid(pid);

            let tx_header = TxHeader {
                typ: P::PACKET_TYPE,
                qos: packet.get_qos(),
                pid,
            };

            debug!("Sending packet {:?}", packet);

            let mut grant = tx_prod
                .grant(packet.packet_len())
                .map_err(|_| Error::StateMismatch)?;

            let mut buf = grant.deref_mut();

            let header_len = tx_header.to_bytes(&mut buf);
            let mut encoder = MqttEncoder::new(&mut buf[header_len..]);
            packet.to_buffer(&mut encoder)?;

            let used = header_len + encoder.bytes().len();
            grant.commit(used);

            pid
        };

        poll_fn(|cx| {
            if let Ok(shared) = self.shared.try_lock() {
                let mut state = shared.borrow_mut();

                match P::PACKET_TYPE {
                    PacketType::Publish => {
                        // Check if `pid` has been acked by the broker
                        if !state.outgoing_pub.contains_key(&pid.get()) {
                            state.register_tx_waker(cx);
                            return Poll::Pending;
                        }
                    }
                    PacketType::Subscribe => {
                        // Check if `pid` has been acked by the broker
                        if !state
                            .pending_ack
                            .contains(&PendingAck::Subscribe(pid.get()))
                        {
                            state.register_tx_waker(cx);
                            return Poll::Pending;
                        }
                    }
                    PacketType::Unsubscribe => {
                        // Check if `pid` has been acked by the broker
                        if !state
                            .pending_ack
                            .contains(&PendingAck::Unsubscribe(pid.get()))
                        {
                            state.register_tx_waker(cx);
                            return Poll::Pending;
                        }
                    }
                    _ => {}
                }

                return Poll::Ready(());
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;

        Ok(pid)
    }

    /// Publish a message to the broker.
    ///
    /// If `QoS` is set to `QoS1` or `QoS2`, this function will wait until the
    /// corresponding `PubAck` message has been received.
    pub async fn publish(&self, packet: impl Into<Publish<'_>>) -> Result<(), Error> {
        let pid = self.send(packet.into()).await?;

        // Wait until `pid` has been ack'd (known by it being removed from the state)
        poll_fn(|cx| {
            if let Ok(shared) = self.shared.try_lock() {
                let mut state = shared.borrow_mut();

                // Check if `pid` has been acked by the broker
                if !state.outgoing_pub.contains_key(&pid.get()) {
                    debug!("publish ack {:?}", pid.get());
                    return Poll::Ready(());
                }

                state.register_tx_waker(cx);
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;

        Ok(())
    }

    pub async fn subscribe<'b, const MAX_TOPICS: usize>(
        &'b self,
        packet: impl Into<Subscribe<'_>>,
    ) -> Result<Subscription<'a, 'b, M, SUBS, MAX_TOPICS>, Error> {
        let subscriber = self
            .rx_pubsub
            .framed_subscriber()
            .map_err(|_| Error::StateMismatch)?;

        let subscribe: Subscribe = packet.into();
        let topic_filters = subscribe
            .topics
            .iter()
            .map(|sub| TopicFilter::new(sub.topic_path))
            .collect::<Result<_, _>>()?;

        let pid = self.send(subscribe).await?;

        // Wait until `pid` has been ack'd (known by it being removed from the state)
        let ack_fut = poll_fn(|cx| {
            if let Ok(shared) = self.shared.try_lock() {
                let mut state = shared.borrow_mut();

                // Check if `pid` has been acked by the broker
                if !state
                    .pending_ack
                    .contains(&PendingAck::Subscribe(pid.get()))
                {
                    return Poll::Ready(());
                }

                state.register_tx_waker(cx);
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        });

        // TODO: Proper timeout duration?
        if let Err(TimeoutError) = with_timeout(Duration::from_secs(5), ack_fut).await {
            let shared_ref = self.shared.lock().await;
            let mut shared = shared_ref.borrow_mut();
            // Pop pid from pending_ack
            shared.pending_ack.remove(&PendingAck::Subscribe(pid.get()));

            return Err(Error::Timeout);
        }

        Ok(Subscription {
            subscriber,
            topic_filters,
            _client: self,
        })
    }
}

/// A topic filter.
///
/// An MQTT topic filter is a multi-field string, delimited by forward
/// slashes, '/', in which fields can contain the wildcards:
///     '+' - Matches a single field
///     '#' - Matches all subsequent fields (must be last field in filter)
///
/// It can be used to match against topics.
#[derive(Debug)]
pub struct TopicFilter {
    filter: heapless::String<MAX_TOPIC_LEN>,
    has_wildcards: bool,
}

impl TopicFilter {
    /// Creates a new topic filter from the string.
    /// This can fail if the filter is not correct, such as having a '#'
    /// wildcard in anyplace other than the last field, or if
    pub fn new(filter: &str) -> Result<Self, Error> {
        let filter = heapless::String::<MAX_TOPIC_LEN>::from_str(filter).unwrap();
        let n = filter.len();

        if n == 0 {
            return Err(Error::BadTopicFilter);
        }

        // If the topic contains any wildcards.
        let has_wildcards = match filter.find('#') {
            Some(i) if i < n - 1 => return Err(Error::BadTopicFilter),
            Some(_) => true,
            None => filter.contains('+'),
        };

        Ok(Self {
            filter,
            has_wildcards,
        })
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Determines if the topic matches the filter.
    pub fn is_match(&self, topic: &str) -> bool {
        if self.has_wildcards {
            for matcher in self.filter.split('/').zip_longest(topic.split('/')) {
                match matcher {
                    EitherOrBoth::Both(filter, field) => {
                        if filter == "#" {
                            break;
                        } else if filter != "+" && filter != field {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }

            true
        } else {
            self.filter == topic
        }
    }
}

impl core::fmt::Display for TopicFilter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.filter())
    }
}

pub struct Subscription<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize = 5> {
    topic_filters: heapless::Vec<TopicFilter, MAX_TOPICS>,
    subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, SUBS>,
    _client: &'b MqttClient<'a, M, SUBS>,
}

impl<'a, 'b, M: RawMutex, const SUBS: usize> futures::Stream for Subscription<'a, 'b, M, SUBS> {
    type Item = Message<'a, M, SUBS>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if let Some(grant) = self.subscriber.read() {
            if let Some(mut msg) = Message::try_new(grant) {
                if self
                    .topic_filters
                    .iter()
                    .any(|filter| filter.is_match(msg.topic_name()))
                {
                    msg.auto_release();

                    return Poll::Ready(Some(msg));
                }
            }
        }

        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize> Drop
    for Subscription<'a, 'b, M, SUBS, MAX_TOPICS>
{
    fn drop(&mut self) {
        // FIXME: UNSUBSCRIBE
    }
}

pub struct Message<'a, M: RawMutex, const SUBS: usize> {
    grant: FrameGrantR<'a, M, SliceBufferProvider<'a>, SUBS>,
    header: FixedHeader,
    qos_pid: QosPid,
    topic_name: Range<usize>,
    payload: Range<usize>,
    properties: Range<usize>,
}

impl<'a, M: RawMutex, const SUBS: usize> Message<'a, M, SUBS> {
    fn try_new(grant: FrameGrantR<'a, M, SliceBufferProvider<'a>, SUBS>) -> Option<Self> {
        let mut decoder = MqttDecoder::new(grant.deref());
        // FIXME: would returning result here be more intuitive?
        let header = decoder.read_fixed_header().ok()?;
        decoder.check_remaining(header.remaining_len).ok()?;

        let topic_name = {
            let topic_name_start = decoder.offset();
            decoder.read_str().ok()?;
            topic_name_start..decoder.offset()
        };

        let qos_pid = match header.qos {
            QoS::AtMostOnce => QosPid::AtMostOnce,
            QoS::AtLeastOnce => QosPid::AtLeastOnce(Pid::try_from(decoder.read_u16().ok()?).ok()?),
            #[cfg(feature = "qos2")]
            QoS::ExactlyOnce => QosPid::ExactlyOnce(Pid::try_from(decoder.read_u16().ok()?).ok()?),
        };

        #[cfg(feature = "mqttv5")]
        let properties = {
            let properties_start = decoder.offset();
            decoder.read_properties().ok()?;
            properties_start..decoder.offset()
        };

        let payload = {
            let payload_start = decoder.offset();
            decoder.read_payload().ok()?;
            payload_start..decoder.offset()
        };

        Some(Self {
            grant,
            header,
            qos_pid,
            topic_name,
            #[cfg(feature = "mqttv5")]
            properties,
            payload,
        })
    }

    fn auto_release(&mut self) {
        self.grant.auto_release(true)
    }

    pub fn dup(&self) -> bool {
        self.header.dup
    }

    pub fn retain(&self) -> bool {
        self.header.retain
    }

    pub fn qos_pid(&self) -> QosPid {
        self.qos_pid
    }

    pub fn topic_name(&self) -> &str {
        // # Safety: Checked at instantiation in `try_new`
        core::str::from_utf8(&self.grant.deref()[self.topic_name.clone()]).unwrap()
    }

    #[cfg(feature = "mqttv5")]
    pub fn properties(&self) -> Properties<'_> {
        Properties::DataBlock(&self.grant.deref()[self.properties.clone()])
    }

    pub fn payload(&self) -> &[u8] {
        // # Safety: Checked at instantiation in `try_new`
        &self.grant.deref()[self.payload.clone()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonwild_topic_filter() {
        const FILTER: &str = "some/topic";

        let filter = TopicFilter::new(FILTER).unwrap();
        assert!(filter.is_match(FILTER));

        assert_eq!(filter.filter(), FILTER);
    }

    #[test]
    fn test_topic_filter() {
        const FILTER1: &str = "some/topic/#";

        let filter = TopicFilter::new(FILTER1).unwrap();
        assert!(filter.is_match("some/topic/thing"));

        assert_eq!(filter.filter(), FILTER1);

        const FILTER2: &str = "some/+/thing";
        let filter = TopicFilter::new(FILTER2).unwrap();
        assert!(filter.is_match("some/topic/thing"));
        assert!(!filter.is_match("some/topic/plus/thing"));

        assert_eq!(filter.filter(), FILTER2);

        const FILTER3: &str = "some/+";
        let filter = TopicFilter::new(FILTER3).unwrap();
        assert!(filter.is_match("some/thing"));
        assert!(!filter.is_match("some/thing/plus"));
    }
}
