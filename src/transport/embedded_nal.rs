use embedded_io_async::{Error as _, ErrorKind};
use embedded_nal_async::TcpConnect;

use super::Transport;

use crate::{error::ConnectionError, Broker, StateError};

// embedded-nal-async Transport
pub struct NalTransport<'a, N: TcpConnect, B: Broker> {
    network: &'a N,
    broker: B,
    socket: Option<N::Connection<'a>>,
}

impl<'a, N: TcpConnect, B: Broker> NalTransport<'a, N, B> {
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

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        self.socket.take();
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.socket.is_some()
    }

    fn socket(&mut self) -> Result<&mut Self::Socket, StateError> {
        self.socket
            .as_mut()
            .ok_or(StateError::Io(ErrorKind::NotConnected))
    }
}
