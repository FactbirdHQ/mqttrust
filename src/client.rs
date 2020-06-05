use crate::alloc::string::ToString;
use core::cell::RefCell;

use heapless::{spsc::Producer, ArrayLength, String, Vec};

use crate::{
    requests::PublishPayload, PublishRequest, QoS, Request, SubscribeRequest, SubscribeTopic,
    UnsubscribeRequest,
};

#[derive(Debug, Clone)]
pub enum MqttClientError {
    Busy,
    Full,
}

pub trait Mqtt<P: PublishPayload = alloc::vec::Vec<u8>> {

    fn send(&self, request: Request<P>) -> Result<(), MqttClientError>;

    fn client_id(&self) -> &str;

    fn publish<T: ArrayLength<u8>>(
        &self,
        topic_name: String<T>,
        payload: P,
        qos: QoS,
    ) -> Result<(), MqttClientError> {
        let req: Request<P> = PublishRequest {
            dup: false,
            qos,
            retain: false,
            topic_name: topic_name.to_string(),
            payload: payload.into(),
        }
        .into();

        self.send(req)
    }

    fn subscribe<L: ArrayLength<SubscribeTopic>>(
        &self,
        topics: Vec<SubscribeTopic, L>,
    ) -> Result<(), MqttClientError> {
        let req: Request<P> = SubscribeRequest {
            topics: topics.to_vec(),
        }
        .into();
        self.send(req)
    }

    fn unsubscribe<L: ArrayLength<alloc::string::String>>(
        &self,
        topics: Vec<alloc::string::String, L>,
    ) -> Result<(), MqttClientError> {
        let req: Request<P> = UnsubscribeRequest {
            topics: topics.to_vec(),
        }
        .into();
        self.send(req)
    }
}

pub struct MqttClient<'a, L, M, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
    M: ArrayLength<u8>,
{
    client_id: String<M>,
    producer: RefCell<Producer<'a, Request<P>, L>>,
}

impl<'a, L, M, P> MqttClient<'a, L, M, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
    M: ArrayLength<u8>,
{
    pub fn new(producer: Producer<'a, Request<P>, L>, client_id: String<M>) -> Self {
        MqttClient {
            client_id,
            producer: RefCell::new(producer),
        }
    }
}

impl<'a, L, M, P> Mqtt<P> for MqttClient<'a, L, M, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
    M: ArrayLength<u8>,
{
    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn send(&self, request: Request<P>) -> Result<(), MqttClientError> {
        self.producer
            .try_borrow_mut()
            .map_err(|_e| MqttClientError::Busy)?
            .enqueue(request)
            .map_err(|_e| MqttClientError::Full)?;
        Ok(())
    }
}
