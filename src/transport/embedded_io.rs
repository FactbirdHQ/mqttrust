use embedded_io_async::{Read, Write};

use crate::error::ConnectionError;

use super::Transport;

/// A transport layer for MQTT using an already connected socket.
///
/// This struct is useful when the socket is already connected and does not require
/// any additional connection logic.
pub struct ConnectedSocketTransport<S>(S);

impl<S: Read + Write> Transport for ConnectedSocketTransport<S> {
    type Socket = S;

    /// This method is a no-op since the socket is already connected.
    ///
    /// # Returns
    ///
    /// `Ok(())` always, as no connection logic is required.
    async fn connect(&mut self) -> Result<(), ConnectionError> {
        Ok(())
    }

    /// This method is a no-op since the socket is managed externally.
    ///
    /// # Returns
    ///
    /// `Ok(())` always, as no disconnection logic is required.
    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        Ok(())
    }

    /// Checks if the transport is currently connected.
    ///
    /// # Returns
    ///
    /// `true` always, as the socket is assumed to be always connected.
    fn is_connected(&self) -> bool {
        true
    }

    /// Provides a mutable reference to the socket used by the transport.
    ///
    /// # Returns
    ///
    /// `Ok(&mut Self::Socket)` always, as the socket is always available.
    fn socket(&mut self) -> Result<&mut Self::Socket, crate::StateError> {
        Ok(&mut self.0)
    }
}
