use bbqueue::framed::FrameProducer;
use core::cell::RefCell;
use core::ops::DerefMut;
use mqttrust::{
    encoding::v4::{encoder::encode_slice, Packet},
    Mqtt, MqttError,
};
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
pub struct Client<'a, 'b, const L: usize> {
    client_id: &'b str,
    producer: Option<RefCell<FrameProducer<'a, L>>>,
}

impl<'a, 'b, const L: usize> Client<'a, 'b, L> {
    pub fn new(producer: FrameProducer<'a, L>, client_id: &'b str) -> Self {
        Self {
            client_id,
            producer: Some(RefCell::new(producer)),
        }
    }

    /// Release `FrameProducer`
    ///
    /// This can be used before dropping `Client` to get back original `FrameProducer`.
    pub fn release_producer(&mut self) -> Option<FrameProducer<'a, L>> {
        match self.producer.take() {
            Some(prod) => Some(prod.into_inner()),
            None => None,
        }
    }
}

impl<'a, 'b, const L: usize> Mqtt for Client<'a, 'b, L> {
    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn send(&self, packet: Packet<'_>) -> Result<(), MqttError> {
        match &self.producer {
            Some(producer) => {
                let mut prod = producer.try_borrow_mut().map_err(|_| MqttError::Borrow)?;
                let max_size = packet.len();
                let mut grant = prod.grant(max_size).map_err(|_| MqttError::Full)?;
                let len = encode_slice(&packet, grant.deref_mut()).map_err(|_| MqttError::Full)?;
                grant.commit(len);
                Ok(())
            }
            None => Err(MqttError::Unavailable),
        }
    }
}
