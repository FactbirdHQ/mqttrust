use core::{
    future::poll_fn,
    ops::{Deref, DerefMut, Range},
    str::FromStr,
    task::Poll,
};

use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embassy_time::{with_timeout, Duration, TimeoutError};
use itertools::EitherOrBoth;
use itertools::Itertools;

use crate::{
    crate_config::{MAX_CLIENT_ID_LEN, MAX_TOPIC_LEN},
    encoding::{
        decoder::{FixedHeader, MqttDecoder},
        encoder::{MqttEncode, MqttEncoder},
        Pid, Publish, QoS, QosPid, Subscribe,
    },
    error::Error,
    pubsub::{
        framed::{FrameGrantR, FramePublisher, FrameSubscriber},
        PubSubChannel, SliceBufferProvider,
    },
    state::{AckStatus, Shared},
    PacketType, Properties, ToPayload, Unsubscribe,
};

pub struct MqttClient<'a, M: RawMutex, const SUBS: usize> {
    tx_publisher: Mutex<M, FramePublisher<'a, SliceBufferProvider<'a>, 1>>,
    shared: &'a Mutex<M, Shared<SUBS>>,
    rx_pubsub: &'a PubSubChannel<SliceBufferProvider<'a>, SUBS>,
    client_id: heapless::String<MAX_CLIENT_ID_LEN>,
}

impl<'a, M: RawMutex, const SUBS: usize> MqttClient<'a, M, SUBS> {
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

    async fn send<P: MqttEncode>(&self, mut packet: P, should_await: bool) -> Result<Pid, Error> {
        let pid = {
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
            grant.commit(used);

            match P::PACKET_TYPE {
                PacketType::Publish => {
                    if packet.get_qos() != Some(QoS::AtMostOnce) {
                        debug!("[Publish] Inserting {:?} into inflight_pub", pid);
                        // FIXME: Properly handle error instead of `unwrap`
                        shared.inflight_pub.insert(pid.into()).unwrap();
                    }
                }
                PacketType::Subscribe => {
                    debug!("[Subscribe] Inserting {:?} into pending_ack", pid);
                    // FIXME: Properly handle error instead of `unwrap`
                    shared
                        .ack_status
                        .insert(pid.into(), AckStatus::AwaitingSubAck)
                        .unwrap();
                }
                _ => {}
            }

            shared
                .outgoing_pid
                .insert(pid.get())
                .map_err(|_| Error::MaxInflight)?;

            pid
        };

        if should_await {
            let transmit_fut = poll_fn(|cx| {
                if let Ok(mut shared) = self.shared.try_lock() {
                    if shared.outgoing_pid.contains(&pid.get()) {
                        shared.register_tx_waker(cx);
                        return Poll::Pending;
                    }

                    return Poll::Ready(());
                }
                cx.waker().wake_by_ref();
                Poll::Pending
            });

            let _ = with_timeout(Duration::from_millis(100), transmit_fut).await;
        }

        Ok(pid)
    }

    /// Publish a message to the broker.
    ///
    /// If `QoS` is set to `QoS1` or `QoS2`, this function will wait until the
    /// corresponding `PubAck` message has been received.
    pub async fn publish<P: ToPayload>(
        &self,
        packet: impl Into<Publish<'_, P>>,
    ) -> Result<(), Error> {
        let pub_pkg: Publish<'_, P> = packet.into();
        debug!("[Publish] Sending to {:?}", pub_pkg.topic_name);

        let should_wait_ack = !matches!(pub_pkg.qos, QoS::AtMostOnce);
        let pid = self.send(pub_pkg, should_wait_ack).await?;

        debug!("[Publish] Sent pid {:?}", pid);

        if should_wait_ack {
            // Wait until `pid` has been ack'd (known by it being removed from the state)
            let ack_fut = poll_fn(|cx| {
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
            });

            with_timeout(Duration::from_secs(10), ack_fut)
                .await
                .map_err(|_| Error::Timeout)?;
        }

        debug!("[Publish] Success pid {:?}", pid);

        Ok(())
    }

    pub async fn subscribe<'b, const MAX_TOPICS: usize>(
        &'b self,
        packet: impl Into<Subscribe<'_>>,
    ) -> Result<Subscription<'a, 'b, M, SUBS, MAX_TOPICS>, Error> {
        const { core::assert!(MAX_TOPICS <= crate::crate_config::MAX_SUB_TOPICS_PER_MSG) };

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

        debug!("[Subscribe] Subscribing to {:?}", topic_filters);

        let pid = self.send(subscribe, true).await?;

        debug!("[Subscribe] Sent pid {:?}", pid);

        // Wait until `pid` has been ack'd (known by it being removed from the state)
        let ack_fut = poll_fn(|cx| {
            if let Ok(mut shared) = self.shared.try_lock() {
                // Check if `pid` has been acked by the broker
                if let Some(&AckStatus::Acked(_)) = shared.ack_status.get(&pid.get()) {
                    match shared.ack_status.remove(&pid.get()) {
                        Some(AckStatus::Acked(codes)) => return Poll::Ready(codes),
                        // Should be checked above!
                        _ => panic!(),
                    }
                }

                shared.register_tx_waker(cx);
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        });

        // TODO: Proper timeout duration?
        match with_timeout(Duration::from_secs(5), ack_fut).await {
            Ok(status) => {
                debug!("[Subscribe] Status codes: {:?}", status);
                if status.iter().any(|&c| c >= 0x80) {
                    return Err(Error::BadTopicFilter);
                }
            }
            Err(TimeoutError) => {
                let mut shared = self.shared.lock().await;
                // Pop pid from pending_ack
                shared.ack_status.remove(&pid.get());

                return Err(Error::Timeout);
            }
        }

        debug!("[Subscribe] Success pid {:?}", pid);

        Ok(Subscription {
            subscriber,
            topic_filters,
            client: self,
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TopicFilter {
    filter: heapless::String<{ MAX_TOPIC_LEN }>,
    has_wildcards: bool,
}

impl TopicFilter {
    /// Creates a new topic filter from the string.
    /// This can fail if the filter is not correct, such as having a '#'
    /// wildcard in anyplace other than the last field, or if
    pub fn new(filter: &str) -> Result<Self, Error> {
        let filter =
            heapless::String::<{ MAX_TOPIC_LEN }>::from_str(filter).map_err(|_| Error::Overflow)?;
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

pub struct Subscription<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize> {
    topic_filters: heapless::Vec<TopicFilter, MAX_TOPICS>,
    subscriber: FrameSubscriber<'a, SliceBufferProvider<'a>, SUBS>,
    client: &'b MqttClient<'a, M, SUBS>,
}

impl<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize>
    Subscription<'a, 'b, M, SUBS, MAX_TOPICS>
{
    pub async fn next_message(&mut self) -> Option<Message<'a, SUBS>> {
        // FIXME: Handle unsubscribed from broker by returning `None`
        loop {
            match select(self.subscriber.read_async(), self.client.clean_session()).await {
                Either::First(grant) => {
                    if let Some(mut msg) = Message::try_new(grant.ok()?) {
                        if self
                            .topic_filters
                            .iter()
                            .any(|filter| filter.is_match(msg.topic_name()))
                        {
                            msg.auto_release();

                            return Some(msg);
                        }
                    }
                }
                Either::Second(_) => return None,
            }
        }
    }
}

impl<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize> futures::Stream
    for Subscription<'a, 'b, M, SUBS, MAX_TOPICS>
{
    type Item = Message<'a, SUBS>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // FIXME: Handle unsubscribed from broker by returning `Poll::Ready(None)`
        if let Some(grant) = self.subscriber.read_with_context(Some(cx)) {
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
        Poll::Pending
    }
}

impl<'a, 'b, M: RawMutex, const SUBS: usize, const MAX_TOPICS: usize> Drop
    for Subscription<'a, 'b, M, SUBS, MAX_TOPICS>
{
    fn drop(&mut self) {
        warn!("Dropping subscription! {:?}", self.topic_filters);

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

            // FIXME: Properly handle error instead of `unwrap`
            if let Ok(mut shared) = self.client.shared.try_lock() {
                debug!("[Unsubscribe] Inserting {:?} into pending_ack", pid);

                shared
                    .ack_status
                    .insert(
                        pid.into(),
                        AckStatus::AwaitingUnsubAck(
                            // # Safety: Checked before subscribing (Assert: MAX_TOPICS <= crate::crate_config::MAX_SUB_TOPICS_PER_MSG)
                            heapless::Vec::try_from(self.topic_filters.as_slice()).unwrap(),
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

pub struct Message<'a, const SUBS: usize> {
    grant: FrameGrantR<'a, SliceBufferProvider<'a>, SUBS>,
    header: FixedHeader,
    qos_pid: QosPid,
    topic_name: Range<usize>,
    payload: Range<usize>,
    #[cfg(feature = "mqttv5")]
    properties: Range<usize>,
}

impl<'a, const SUBS: usize> Message<'a, SUBS> {
    fn try_new(grant: FrameGrantR<'a, SliceBufferProvider<'a>, SUBS>) -> Option<Self> {
        let mut decoder = MqttDecoder::try_new(grant.deref()).ok()?;
        // FIXME: would returning result here be more intuitive?

        let header = decoder.fixed_header();
        decoder.check_remaining(header.remaining_len).ok()?;

        let topic_name = {
            let topic_name_start = decoder.offset() + 2;
            let name = decoder.read_str().ok()?;
            warn!("GOT MESSAGE TOPIC: {}", name);
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
        unsafe { core::str::from_utf8_unchecked(&self.grant.deref()[self.topic_name.clone()]) }
    }

    #[cfg(feature = "mqttv5")]
    pub fn properties(&self) -> Properties<'_> {
        Properties::DataBlock(&self.grant.deref()[self.properties.clone()])
    }

    pub fn payload(&self) -> &[u8] {
        // # Safety: Checked at instantiation in `try_new`
        &self.grant.deref()[self.payload.clone()]
    }

    pub fn payload_mut(&mut self) -> &mut [u8] {
        // # Safety: Checked at instantiation in `try_new`
        &mut self.grant.deref_mut()[self.payload.clone()]
    }
}

impl<'a, const SUBS: usize> Deref for Message<'a, SUBS> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl<'a, const SUBS: usize> DerefMut for Message<'a, SUBS> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.payload_mut()
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
        assert!(filter.is_match("some/topic/thing/more/sections"));

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
