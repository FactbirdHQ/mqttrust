use heapless::{spsc::Producer, ArrayLength};
use mqttrust::Mqtt;
use mqttrust::{PublishPayload, Request};

#[derive(Debug, Clone)]
pub enum MqttClientError {
    Full,
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
    producer: Producer<'a, Request<P>, L, u8>,
}

impl<'a, 'b, L, P> MqttClient<'a, 'b, L, P>
where
    P: PublishPayload,
    L: ArrayLength<Request<P>>,
{
    pub fn new(producer: Producer<'a, Request<P>, L, u8>, client_id: &'b str) -> Self {
        MqttClient {
            client_id,
            producer,
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

    fn send(&mut self, request: Request<P>) -> Result<(), Self::Error> {
        self.producer
            .enqueue(request)
            .map_err(|_| MqttClientError::Full)?;
        Ok(())
    }
}
