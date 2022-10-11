use bbqueue::framed::FrameProducer;
use core::cell::RefCell;
use core::ops::DerefMut;
use mqttrust::{
    encoding::v4::{encoder::encode_slice, Packet},
    Mqtt, MqttError,
};
/// MQTT Client
///
/// This client is merely a convenience wrapper around a
/// `heapless::spsc::Producer`, making it easier to send certain MQTT packet
/// types, and maintaining a common reference to a client id. Also it implements
/// the [`Mqtt`] trait.
///
/// **Lifetimes**:
/// - `'a`: Lifetime of the queue for exchanging packets between the client and
///   [Eventloop](crate::eventloop::EventLoop). This must have the same lifetime as the corresponding
///   Consumer. Usually `'static`.
/// - `'b`: Lifetime of `client_id` str.
///
/// **Generics**:
/// - `L`: Length of the queue for exchanging packets between the client and
///   [Eventloop](crate::eventloop::EventLoop).
///   The length is in bytes and it must be chosen long enough to contain serialized MQTT packets and
///   [FrameProducer](bbqueue::framed) header bytes.
///   For example a MQTT packet with 30 bytes payload and 20 bytes topic name takes 59 bytes to serialize
///   into MQTT frame plus ~2 bytes (depending on grant length) for [FrameProducer](bbqueue::framed) header.
///   For rough calculation `payload_len + topic_name + 15` can be used to determine
///   how many bytes one packet consumes.
///   Packets are read out from queue only when [Eventloop::yield_event](crate::eventloop::EventLoop::yield_event) is called.
///   Therefore make sure that queue length is long enough to contain multiple packets if you want to call
///   [send](Client::send) multiple times in the row.
pub struct Client<'a, 'b, const L: usize> {
    client_id: &'b str,
    producer: RefCell<FrameProducer<'a, L>>,
}

impl<'a, 'b, const L: usize> Client<'a, 'b, L> {
    pub fn new(producer: FrameProducer<'a, L>, client_id: &'b str) -> Self {
        Self {
            client_id,
            producer: RefCell::new(producer),
        }
    }
}

impl<'a, 'b, const L: usize> Mqtt for Client<'a, 'b, L> {
    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn send(&self, packet: Packet<'_>) -> Result<(), MqttError> {
        let mut prod = self
            .producer
            .try_borrow_mut()
            .map_err(|_| MqttError::Borrow)?;

        let max_size = packet.len();
        let mut grant = prod.grant(max_size).map_err(|_| MqttError::Full)?;

        let len = encode_slice(&packet, grant.deref_mut()).map_err(|_| MqttError::Full)?;

        grant.commit(len);

        Ok(())
    }
}
