use alloc::{string::String, vec::Vec};
use mqttrs::{Connect, QoS, SubscribeTopic};

pub trait PublishPayload {
    fn as_vec(self) -> Vec<u8>;
}

impl PublishPayload for Vec<u8> {
    fn as_vec(self) -> Vec<u8> {
        self
    }
}

impl<L> PublishPayload for heapless::Vec<u8, L>
where
    L: heapless::ArrayLength<u8>,
{
    fn as_vec(self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&self);
        v
    }
}

/// Publish request ([MQTT 3.3]).
///
/// [MQTT 3.3]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718037
#[derive(Debug, Clone, PartialEq)]
pub struct PublishRequest<P = Vec<u8>> {
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
    pub topic_name: String,
    pub payload: P,
}

impl<P> PublishRequest<P>
where
    P: PublishPayload,
{
    pub fn new(topic_name: String, payload: P) -> Self {
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
    pub topics: Vec<SubscribeTopic>,
}

/// Unsubscribe request ([MQTT 3.10]).
///
/// [MQTT 3.10]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718072
#[derive(Debug, Clone, PartialEq)]
pub struct UnsubscribeRequest {
    pub topics: Vec<String>,
}

/// Requests by the client to mqtt event loop. Request are
/// handle one by one
#[derive(Debug, Clone, PartialEq)]
pub enum Request<P = Vec<u8>>
where
    P: PublishPayload,
{
    Publish(PublishRequest<P>),
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
    Reconnect(Connect),
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
