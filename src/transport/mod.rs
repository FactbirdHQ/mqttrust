pub mod embedded_nal;

#[cfg(feature = "embedded-tls")]
pub mod embedded_tls;

use embedded_io_async::{Read, Write};

use crate::{
    encoder::MqttEncode, error::ConnectionError, received_packet::ReceivedPacket, Broker,
    StateError,
};

pub trait Transport {
    type Socket: Read + Write;

    async fn connect(&mut self, broker: &mut impl Broker) -> Result<(), ConnectionError>;

    fn disconnect(&mut self) -> Result<(), ConnectionError>;

    fn is_connected(&self) -> bool;

    async fn write_packet(&mut self, packet: impl MqttEncode) -> Result<(), StateError>;

    async fn write(&mut self, buf: &[u8]) -> Result<(), StateError>;

    async fn get_received_packet(&mut self)
        -> Result<ReceivedPacket<'_, Self::Socket>, StateError>;
}

/// Blanket implementation for mutable references of `impl Transport`
impl<T: Transport> Transport for &mut T {
    type Socket = T::Socket;

    async fn connect(&mut self, broker: &mut impl Broker) -> Result<(), ConnectionError> {
        T::connect(self, broker).await
    }

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        T::disconnect(self)
    }

    fn is_connected(&self) -> bool {
        T::is_connected(self)
    }

    async fn write_packet(&mut self, packet: impl MqttEncode) -> Result<(), StateError> {
        T::write_packet(self, packet).await
    }

    async fn write(&mut self, buf: &[u8]) -> Result<(), StateError> {
        T::write(self, buf).await
    }

    async fn get_received_packet(
        &mut self,
    ) -> Result<ReceivedPacket<'_, Self::Socket>, StateError> {
        T::get_received_packet(self).await
    }
}
