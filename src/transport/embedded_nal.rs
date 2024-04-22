use embedded_io_async::{Error as _, ErrorKind, Write};
use embedded_nal_async::TcpConnect;

use super::Transport;

use crate::{
    encoder::{MqttEncode, MqttEncoder},
    error::ConnectionError,
    packet::PacketBuffer,
    received_packet::ReceivedPacket,
    Broker, StateError,
};

// embedded-nal-async Transport
pub struct NalTransport<'a, N: TcpConnect> {
    network: &'a N,
    socket: Option<N::Connection<'a>>,
    packet_buf: PacketBuffer<128>,
}

impl<'a, N: TcpConnect> NalTransport<'a, N> {
    pub fn new(network: &'a N) -> Self {
        Self {
            network,
            socket: None,
            packet_buf: PacketBuffer::new(),
        }
    }
}

impl<'a, N: TcpConnect> Transport for NalTransport<'a, N> {
    type Socket = N::Connection<'a>;

    async fn connect(&mut self, broker: &mut impl Broker) -> Result<(), ConnectionError> {
        let addr = broker
            .get_address()
            .await
            .ok_or(ConnectionError::InvalidAddress)?;

        let socket = self
            .network
            .connect(addr)
            .await
            .map_err(|e| ConnectionError::Io(e.kind()))?;

        self.socket.replace(socket);

        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        self.socket.take();
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.socket.is_some()
    }

    async fn write_packet(&mut self, packet: impl MqttEncode) -> Result<(), StateError> {
        // FIXME: Reuse packet buffer?
        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        packet
            .to_buffer(&mut encoder)
            .map_err(|_| StateError::Deserialization)?;
        self.write(encoder.packet_bytes()).await?;
        Ok(())
    }

    async fn write(&mut self, buf: &[u8]) -> Result<(), StateError> {
        let Some(ref mut socket) = self.socket else {
            return Err(StateError::Io(ErrorKind::NotConnected));
        };

        trace!("Writing {} bytes to socket", buf.len());
        socket
            .write_all(buf)
            .await
            .map_err(|e| StateError::Io(e.kind()))?;
        socket.flush().await.map_err(|e| StateError::Io(e.kind()))
    }

    async fn get_received_packet(
        &mut self,
    ) -> Result<ReceivedPacket<'_, Self::Socket>, StateError> {
        if !self.is_connected() {
            return Err(StateError::Io(ErrorKind::NotConnected));
        };

        while !self.packet_buf.packet_available() {
            self.packet_buf
                .receive(self.socket.as_mut().unwrap())
                .await
                .map_err(|kind| {
                    error!("DISCONNECTING {:?}", kind);
                    self.socket.take();
                    StateError::Io(kind)
                })?;
        }

        self.packet_buf
            .received_packet(self.socket.as_mut().unwrap())
            .map_err(|_| StateError::Deserialization)
    }
}
