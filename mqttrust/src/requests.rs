use heapless::{String, Vec};
use mqttrs::{QoS, SubscribeTopic};

/// Publish request ([MQTT 3.3]).
///
/// [MQTT 3.3]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718037
#[derive(Debug, Clone, PartialEq)]
pub struct PublishRequest<'a> {
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
    pub topic_name: &'a str,
    pub payload: &'a [u8],
}

impl<'a> PublishRequest<'a> {
    pub fn new(topic_name: &'a str, payload: &'a [u8]) -> Self {
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
    pub topics: Vec<SubscribeTopic, 5>,
}

/// Unsubscribe request ([MQTT 3.10]).
///
/// [MQTT 3.10]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718072
#[derive(Debug, Clone, PartialEq)]
pub struct UnsubscribeRequest {
    pub topics: Vec<String<256>, 5>,
}

/// Requests by the client to mqtt event loop. Request are
/// handle one by one
#[derive(Debug, Clone, PartialEq)]
pub enum Request<'a> {
    Publish(PublishRequest<'a>),
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
    // Reconnect(Connect),
    Disconnect,
}

impl<'a> From<PublishRequest<'a>> for Request<'a> {
    fn from(publish: PublishRequest<'a>) -> Self {
        Request::Publish(publish)
    }
}

impl<'a> From<SubscribeRequest> for Request<'a> {
    fn from(subscribe: SubscribeRequest) -> Self {
        Request::Subscribe(subscribe)
    }
}

impl<'a> From<UnsubscribeRequest> for Request<'a> {
    fn from(unsubscribe: UnsubscribeRequest) -> Self {
        Request::Unsubscribe(unsubscribe)
    }
}
