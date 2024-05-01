use embedded_io_async::{Error as _, ErrorKind};
use embedded_nal_async::TcpConnect;

use super::Transport;

use crate::{error::ConnectionError, Broker, StateError};

// embedded-nal-async Transport
pub struct NalTransport<'a, N: TcpConnect> {
    network: &'a N,
    socket: Option<N::Connection<'a>>,
}

impl<'a, N: TcpConnect> NalTransport<'a, N> {
    pub fn new(network: &'a N) -> Self {
        Self {
            network,
            socket: None,
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

    fn socket(&mut self) -> Result<&mut Self::Socket, StateError> {
        self.socket
            .as_mut()
            .ok_or(StateError::Io(ErrorKind::NotConnected))
    }
}
