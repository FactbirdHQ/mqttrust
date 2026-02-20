pub mod embedded_io;
pub mod embedded_nal;

#[cfg(feature = "embedded-tls")]
pub mod embedded_tls;

use core::future::Future;

use embedded_io_async::{Read, Write};

use crate::{error::ConnectionError, StateError};

/// A trait representing a transport layer for MQTT communication.
///
/// This trait defines the necessary methods for establishing, managing, and using a transport
/// connection, such as TCP or TLS, for MQTT communication.
pub trait Transport {
    /// The type of socket used by the transport, which must implement both `Read` and `Write`.
    type Socket: Read + Write;

    /// Asynchronously connects to the transport.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the connection was successful, otherwise a `ConnectionError`.
    async fn connect(&mut self) -> Result<(), ConnectionError>;

    /// Disconnects from the transport.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the disconnection was successful, otherwise a `ConnectionError`.
    fn disconnect(&mut self) -> Result<(), ConnectionError>;

    /// Checks if the transport is currently connected.
    ///
    /// # Returns
    ///
    /// `true` if the transport is connected, otherwise `false`.
    fn is_connected(&self) -> bool;

    /// Provides a mutable reference to the socket used by the transport.
    ///
    /// # Returns
    ///
    /// `Ok(&mut Self::Socket)` if the socket is available, otherwise a `StateError`.
    fn socket(&mut self) -> Result<&mut Self::Socket, StateError>;
}

/// Blanket implementation for mutable references of `impl Transport`.
///
/// This allows any mutable reference to a type that implements `Transport` to also implement
/// the `Transport` trait.
impl<T: Transport> Transport for &mut T {
    type Socket = T::Socket;

    fn connect(&mut self) -> impl Future<Output = Result<(), ConnectionError>> {
        T::connect(self)
    }

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        T::disconnect(self)
    }

    fn is_connected(&self) -> bool {
        T::is_connected(self)
    }

    fn socket(&mut self) -> Result<&mut Self::Socket, StateError> {
        T::socket(self)
    }
}
