use crate::{Mqtt, MqttError, PublishPayload, Request};
use core::cell::RefCell;
use heapless::spsc::Producer;

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
/// - 'b: The lifetime of client id str
///
/// **Generics**:
/// - L: The length of the queue, exhanging packets between the client and the
///   event loop. Length in number of request packets
/// - P: The type of the payload field of publish requests. This **must**
///   implement the [`PublishPayload`] trait
pub struct Client<'a, 'b, P, const L: usize>
where
    P: PublishPayload,
{
    client_id: &'b str,
    producer: RefCell<Producer<'a, Request<P>, L>>,
}

impl<'a, 'b, P, const L: usize> Client<'a, 'b, P, L>
where
    P: PublishPayload,
{
    pub fn new(producer: Producer<'a, Request<P>, L>, client_id: &'b str) -> Self {
        Self {
            client_id,
            producer: RefCell::new(producer),
        }
    }
}

impl<'a, 'b, P, const L: usize> Mqtt<P> for Client<'a, 'b, P, L>
where
    P: PublishPayload,
{
    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn send(&self, request: Request<P>) -> Result<(), MqttError> {
        self.producer
            .try_borrow_mut()
            .map_err(|_| MqttError::Borrow)?
            .enqueue(request)
            .map_err(|_| MqttError::Full)?;
        Ok(())
    }
}
