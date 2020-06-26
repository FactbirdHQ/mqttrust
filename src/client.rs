use crate::{
    requests::PublishPayload, PublishRequest, QoS, Request, SubscribeRequest, SubscribeTopic,
    UnsubscribeRequest,
};
use core::cell::RefCell;
use heapless::{consts, spsc::Producer, ArrayLength, String, Vec};

#[derive(Debug, Clone)]
pub enum MqttClientError {
    Busy,
    Full,
}

pub trait Mqtt<P: PublishPayload> {
    type Error;

    fn send(&self, request: Request<P>) -> Result<(), Self::Error>;

    fn client_id(&self) -> &str;

    fn publish(
        &self,
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

    fn subscribe(&self, topics: Vec<SubscribeTopic, consts::U5>) -> Result<(), Self::Error> {
        let req: Request<P> = SubscribeRequest { topics }.into();
        self.send(req)
    }

    fn unsubscribe(
        &self,
        topics: Vec<String<consts::U256>, consts::U5>,
    ) -> Result<(), Self::Error> {
        let req: Request<P> = UnsubscribeRequest { topics }.into();
        self.send(req)
    }
}

/// MQTT Client
///
/// This client is meerly a convenience wrapper around a
/// `heapless::spsc::Producer`, making it easier to send certain MQTT packet
/// types, and maintaining a common reference to a client id. Also it implements
/// the [`Mqtt`] trait.
///
/// **Lifetimes**:
/// - 'a: The lifetime of the queue exhanging packets between the client and the
///   event loop. This must have the same lifetime as the corresponding
///   Consumer. Usually 'static.
/// - 'b: The lifetime of the packet fields, backed by a slice buffer
///
/// **Generics**:
/// - L: The length of the queue, exhanging packets between the client and the
///   event loop. Length in number of request packets
/// - P: The type of the payload field of publish requests. This **must**
///   implement the [`PublishPayload`] trait
pub struct MqttClient<'a, 'b, L, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
{
    client_id: &'b str,
    producer: RefCell<Producer<'a, Request<P>, L, u8>>,
}

impl<'a, 'b, L, P> MqttClient<'a, 'b, L, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
{
    pub fn new(producer: Producer<'a, Request<P>, L, u8>, client_id: &'b str) -> Self {
        MqttClient {
            client_id,
            producer: RefCell::new(producer),
        }
    }
}

impl<'a, 'b, L, P> Mqtt<P> for MqttClient<'a, 'b, L, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
{
    type Error = MqttClientError;

    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn send(&self, request: Request<P>) -> Result<(), Self::Error> {
        self.producer
            .try_borrow_mut()
            .map_err(|_e| MqttClientError::Busy)?
            .enqueue(request)
            .map_err(|_e| MqttClientError::Full)?;
        Ok(())
    }
}
