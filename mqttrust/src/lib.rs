#![cfg_attr(not(test), no_std)]

mod requests;

use heapless::{String, Vec};
pub use mqttrs::{QoS, SubscribeTopic};
pub use requests::{PublishPayload, PublishRequest, Request, SubscribeRequest, UnsubscribeRequest};

#[derive(Debug, Clone)]
pub enum MqttError {
    Full,
    Borrow,
}

pub trait Mqtt<P: PublishPayload> {
    fn send(&self, request: Request<P>) -> Result<(), MqttError>;

    fn client_id(&self) -> &str;

    fn publish(&self, topic_name: String<256>, payload: P, qos: QoS) -> Result<(), MqttError> {
        let req: Request<P> = PublishRequest {
            dup: false,
            qos,
            retain: false,
            topic_name,
            payload,
        }
        .into();

        self.send(req)
    }

    fn subscribe(&self, topics: Vec<SubscribeTopic, 5>) -> Result<(), MqttError> {
        let req: Request<P> = SubscribeRequest { topics }.into();
        self.send(req)
    }

    fn unsubscribe(&self, topics: Vec<String<256>, 5>) -> Result<(), MqttError> {
        let req: Request<P> = UnsubscribeRequest { topics }.into();
        self.send(req)
    }
}
