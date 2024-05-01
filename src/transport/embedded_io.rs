use embedded_io_async::{Read, Write};

use crate::{error::ConnectionError, Broker};

use super::Transport;

pub struct ConnectedSocketTransport<S>(S);

impl<S: Read + Write> Transport for ConnectedSocketTransport<S> {
    type Socket = S;

    async fn connect(&mut self, _broker: &mut impl Broker) -> Result<(), ConnectionError> {
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        Ok(())
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn socket(&mut self) -> Result<&mut Self::Socket, crate::StateError> {
        Ok(&mut self.0)
    }
}
