use crate::alloc::string::ToString;
use core::cell::RefCell;

use heapless::{spsc::Producer, ArrayLength, String, Vec};

use crate::{PublishRequest, QoS, Request, SubscribeRequest, SubscribeTopic, UnsubscribeRequest};

#[derive(Debug)]
pub enum MqttClientError {
    Busy,
    Full,
}

pub trait Mqtt {
    fn send(&self, request: Request) -> Result<(), MqttClientError>;

    fn client_id(&self) -> &str;

    fn publish<T: ArrayLength<u8>, L: ArrayLength<u8>>(
        &self,
        topic_name: String<T>,
        payload: Vec<u8, L>,
        qos: QoS,
    ) -> Result<(), MqttClientError> {
        self.send(
            PublishRequest {
                dup: false,
                qos,
                retain: false,
                topic_name: topic_name.to_string(),
                payload: payload.to_vec(),
            }
            .into(),
        )
    }

    fn subscribe<L: ArrayLength<SubscribeTopic>>(
        &self,
        topics: Vec<SubscribeTopic, L>,
    ) -> Result<(), MqttClientError> {
        self.send(
            SubscribeRequest {
                topics: topics.to_vec(),
            }
            .into(),
        )
    }

    fn unsubscribe<L: ArrayLength<alloc::string::String>>(
        &self,
        topics: Vec<alloc::string::String, L>,
    ) -> Result<(), MqttClientError> {
        self.send(
            UnsubscribeRequest {
                topics: topics.to_vec(),
            }
            .into(),
        )
    }
}

pub struct MqttClient<'a, L, M>
where
    L: ArrayLength<Request>,
    M: ArrayLength<u8>,
{
    client_id: String<M>,
    producer: RefCell<Producer<'a, Request, L>>,
}

impl<'a, L, M> MqttClient<'a, L, M>
where
    L: ArrayLength<Request>,
    M: ArrayLength<u8>,
{
    pub fn new(producer: Producer<'a, Request, L>, client_id: String<M>) -> Self {
        MqttClient {
            client_id,
            producer: RefCell::new(producer),
        }
    }
}

impl<'a, L, M> Mqtt for MqttClient<'a, L, M>
where
    L: ArrayLength<Request>,
    M: ArrayLength<u8>,
{
    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn send(&self, request: Request) -> Result<(), MqttClientError> {
        self.producer
            .try_borrow_mut()
            .map_err(|_e| MqttClientError::Busy)?
            .enqueue(request)
            .map_err(|_e| MqttClientError::Full)?;
        Ok(())
    }
}
