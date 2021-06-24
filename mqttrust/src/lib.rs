#![cfg_attr(not(test), no_std)]

mod requests;

use heapless::{String, Vec};
pub use mqttrs::{QoS, SubscribeTopic};
pub use requests::{PublishRequest, Request, SubscribeRequest, UnsubscribeRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttError {
    Full,
    Borrow,
    Overflow,
}

pub trait Mqtt {
    fn send(&self, request: Request<'_>) -> Result<(), MqttError>;

    fn client_id(&self) -> &str;

    fn publish(&self, topic_name: &str, payload: &[u8], qos: QoS) -> Result<(), MqttError> {
        let req: Request = PublishRequest {
            dup: false,
            qos,
            retain: false,
            topic_name,
            payload,
        }
        .into();

        self.send(req)
    }

    fn subscribe(&self, topic: SubscribeTopic) -> Result<(), MqttError> {
        let req: Request = SubscribeRequest {
            topics: Vec::from_slice(&[topic]).unwrap(),
        }
        .into();
        self.send(req)
    }

    fn subscribe_many(&self, topics: Vec<SubscribeTopic, 5>) -> Result<(), MqttError> {
        let req: Request = SubscribeRequest { topics }.into();
        self.send(req)
    }

    fn unsubscribe(&self, topic: String<256>) -> Result<(), MqttError> {
        let req: Request = UnsubscribeRequest {
            topics: Vec::from_slice(&[topic]).unwrap(),
        }
        .into();
        self.send(req)
    }

    fn unsubscribe_many(&self, topics: Vec<String<256>, 5>) -> Result<(), MqttError> {
        let req: Request = UnsubscribeRequest { topics }.into();
        self.send(req)
    }
}
