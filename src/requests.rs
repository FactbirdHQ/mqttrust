use heapless::{consts, ArrayLength, String, Vec};
use mqttrs::{QoS, SubscribeTopic};

pub trait PublishPayload {
    fn as_bytes(&self, buffer: &mut [u8]) -> Result<usize, ()>;
    fn from_bytes(v: &[u8]) -> Self;
}

#[cfg(any(test, feature = "alloc"))]
impl PublishPayload for alloc::vec::Vec<u8> {
    fn as_bytes(&self, buffer: &mut [u8]) -> Result<usize, ()> {
        let len = self.len();
        if buffer.len() < len {
            return Err(());
        }

        buffer[..len].copy_from_slice(self.as_slice());
        Ok(len)
    }

    fn from_bytes(v: &[u8]) -> Self {
        let mut vec = alloc::vec::Vec::new();
        vec.extend_from_slice(v);
        vec
    }
}

impl<L> PublishPayload for Vec<u8, L>
where
    L: ArrayLength<u8>,
{
    fn as_bytes(&self, buffer: &mut [u8]) -> Result<usize, ()> {
        let len = self.len();
        if buffer.len() < len {
            return Err(());
        }

        buffer[..len].copy_from_slice(self.as_ref());
        Ok(len)
    }

    fn from_bytes(v: &[u8]) -> Self {
        Vec::from_slice(v).expect("Failed to do from_slice! [from_bytes()]")
    }
}

/// Publish request ([MQTT 3.3]).
///
/// [MQTT 3.3]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718037
#[derive(Debug, Clone, PartialEq)]
pub struct PublishRequest<P> {
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
    pub topic_name: String<consts::U256>,
    pub payload: P,
}

impl<P> PublishRequest<P>
where
    P: PublishPayload,
{
    pub fn new(topic_name: String<consts::U256>, payload: P) -> Self {
        PublishRequest {
            dup: false,
            qos: QoS::AtLeastOnce,
            retain: false,
            topic_name,
            payload,
        }
    }

    pub fn qos(self, qos: QoS) -> Self {
        Self { qos, ..self }
    }
}

/// Subscribe request ([MQTT 3.8]).
///
/// [MQTT 3.8]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718063
#[derive(Debug, Clone, PartialEq)]
pub struct SubscribeRequest {
    pub topics: Vec<SubscribeTopic, consts::U5>,
}


/// Unsubscribe request ([MQTT 3.10]).
///
/// [MQTT 3.10]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718072
#[derive(Debug, Clone, PartialEq)]
pub struct UnsubscribeRequest {
    pub topics: Vec<String<consts::U256>, consts::U5>,
}

/// Requests by the client to mqtt event loop. Request are
/// handle one by one
#[derive(Debug, Clone, PartialEq)]
pub enum Request<P>
where
    P: PublishPayload,
{
    Publish(PublishRequest<P>),
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
    // Reconnect(Connect),
    Disconnect,
}

impl<P> From<PublishRequest<P>> for Request<P>
where
    P: PublishPayload,
{
    fn from(publish: PublishRequest<P>) -> Self {
        Request::Publish(publish)
    }
}

impl<P> From<SubscribeRequest> for Request<P>
where
    P: PublishPayload,
{
    fn from(subscribe: SubscribeRequest) -> Self {
        Request::Subscribe(subscribe)
    }
}

impl<P> From<UnsubscribeRequest> for Request<P>
where
    P: PublishPayload,
{
    fn from(unsubscribe: UnsubscribeRequest) -> Self {
        Request::Unsubscribe(unsubscribe)
    }
}
