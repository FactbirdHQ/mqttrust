use crate::alloc::string::ToString;
use core::cell::RefCell;
use alloc::{vec::Vec, string::String};

use heapless::{spsc::Producer, ArrayLength};

use crate::{
    requests::PublishPayload, PublishRequest, QoS, Request, SubscribeRequest, SubscribeTopic,
    UnsubscribeRequest,
};

#[derive(Debug, Clone)]
pub enum MqttClientError {
    Busy,
    Full,
}

pub trait Mqtt<P: PublishPayload = Vec<u8>>
where
    P: PublishPayload
{

    fn send(&self, request: Request<P>) -> Result<(), MqttClientError>;

    fn client_id(&self) -> &str;

    fn publish(
        &self,
        topic_name: String,
        payload: P,
        qos: QoS,
    ) -> Result<(), MqttClientError> {
        let req: Request<P> = PublishRequest {
            dup: false,
            qos,
            retain: false,
            topic_name,
            payload: payload.into(),
        }
        .into();

        self.send(req)
    }

    fn subscribe(
        &self,
        topics: Vec<SubscribeTopic>,
    ) -> Result<(), MqttClientError> {
        let req: Request<P> = SubscribeRequest {
            topics: topics.to_vec(),
        }
        .into();
        self.send(req)
    }

    fn unsubscribe(
        &self,
        topics: Vec<String>,
    ) -> Result<(), MqttClientError> {
        let req: Request<P> = UnsubscribeRequest {
            topics: topics.to_vec(),
        }
        .into();
        self.send(req)
    }
}

pub struct MqttClient<'a, L, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
{
    client_id: String,
    producer: RefCell<Producer<'a, Request<P>, L>>,
}

impl<'a, L, P> MqttClient<'a, L, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
{
    pub fn new(producer: Producer<'a, Request<P>, L>, client_id: String) -> Self {
        MqttClient {
            client_id,
            producer: RefCell::new(producer),
        }
    }
}

impl<'a, L, P> Mqtt<P> for MqttClient<'a, L, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
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
