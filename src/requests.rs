use alloc::{string::String, vec::Vec};
use mqttrs::{Connect, QoS, SubscribeTopic};

/// Publish request ([MQTT 3.3]).
///
/// [MQTT 3.3]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718037
#[derive(Debug, Clone, PartialEq)]
pub struct PublishRequest {
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
    pub topic_name: String,
    pub payload: Vec<u8>,
}

impl PublishRequest {
    pub fn new(topic_name: String, payload: Vec<u8>) -> Self {
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
pub enum Request {
    Publish(PublishRequest),
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
    Reconnect(Connect),
    Disconnect,
}

impl From<PublishRequest> for Request {
    fn from(publish: PublishRequest) -> Self {
        Request::Publish(publish)
    }
}

impl From<SubscribeRequest> for Request {
    fn from(subscribe: SubscribeRequest) -> Self {
        Request::Subscribe(subscribe)
    }
}

impl From<UnsubscribeRequest> for Request {
    fn from(unsubscribe: UnsubscribeRequest) -> Self {
        Request::Unsubscribe(unsubscribe)
    }
}
