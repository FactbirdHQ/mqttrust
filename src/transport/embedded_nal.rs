use embedded_io_async::{Error as _, ErrorKind};
use embedded_nal_async::TcpConnect;

use super::Transport;

use crate::{error::ConnectionError, Broker, StateError};

/// A transport layer for MQTT using a network abstraction layer (NAL).
///
/// This struct manages the connection to the MQTT broker using a TCP connection.
pub struct NalTransport<'a, N: TcpConnect, B: Broker> {
    network: &'a N,
    broker: B,
    socket: Option<N::Connection<'a>>,
}

impl<'a, N: TcpConnect, B: Broker> NalTransport<'a, N, B> {
    /// Creates a new `NalTransport`.
    ///
    /// # Parameters
    ///
    /// - `network`: The network abstraction layer for TCP connections.
    /// - `broker`: The MQTT broker information.
    ///
    /// # Returns
    ///
    /// A new instance of `NalTransport`.
    pub fn new(network: &'a N, broker: B) -> Self {
        Self {
            network,
            broker,
            socket: None,
        }
    }
}

impl<'a, N: TcpConnect, B: Broker> Transport for NalTransport<'a, N, B> {
    type Socket = N::Connection<'a>;

    /// Asynchronously connects to the MQTT broker.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the connection was successful, otherwise a `ConnectionError`.
    async fn connect(&mut self) -> Result<(), ConnectionError> {
        let addr = self
            .broker
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

    /// Disconnects from the MQTT broker.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the disconnection was successful, otherwise a `ConnectionError`.
    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        self.socket.take();
        Ok(())
    }

    /// Checks if the transport is currently connected.
    ///
    /// # Returns
    ///
    /// `true` if the transport is connected, otherwise `false`.
    fn is_connected(&self) -> bool {
        self.socket.is_some()
    }

    /// Provides a mutable reference to the socket used by the transport.
    ///
    /// # Returns
    ///
    /// `Ok(&mut Self::Socket)` if the socket is available, otherwise a `StateError`.
    fn socket(&mut self) -> Result<&mut Self::Socket, StateError> {
        self.socket
            .as_mut()
            .ok_or(StateError::Io(ErrorKind::NotConnected))
    }
}
