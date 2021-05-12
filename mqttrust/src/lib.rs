#![cfg_attr(not(test), no_std)]

mod requests;

use heapless::{consts, String, Vec};
pub use mqttrs::{QoS, SubscribeTopic};
pub use requests::{PublishPayload, PublishRequest, Request, SubscribeRequest, UnsubscribeRequest};

pub trait Mqtt<P: PublishPayload> {
    type Error;

    fn send(&mut self, request: Request<P>) -> Result<(), Self::Error>;

    fn client_id(&self) -> &str;

    fn publish(
        &mut self,
        topic_name: String<consts::U256>,
        payload: P,
        qos: QoS,
    ) -> Result<(), Self::Error> {
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

    fn subscribe(&mut self, topics: Vec<SubscribeTopic, consts::U5>) -> Result<(), Self::Error> {
        let req: Request<P> = SubscribeRequest { topics }.into();
        self.send(req)
    }

    fn unsubscribe(
        &mut self,
        topics: Vec<String<consts::U256>, consts::U5>,
    ) -> Result<(), Self::Error> {
        let req: Request<P> = UnsubscribeRequest { topics }.into();
        self.send(req)
    }
}
