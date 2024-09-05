pub mod embedded_io;
pub mod embedded_nal;

#[cfg(feature = "embedded-tls")]
pub mod embedded_tls;

use embedded_io_async::{Read, Write};

use crate::{error::ConnectionError, StateError};

pub trait Transport {
    type Socket: Read + Write;

    async fn connect(&mut self) -> Result<(), ConnectionError>;

    fn disconnect(&mut self) -> Result<(), ConnectionError>;

    fn is_connected(&self) -> bool;

    fn socket(&mut self) -> Result<&mut Self::Socket, StateError>;
}

/// Blanket implementation for mutable references of `impl Transport`
impl<T: Transport> Transport for &mut T {
    type Socket = T::Socket;

    async fn connect(&mut self) -> Result<(), ConnectionError> {
        T::connect(self).await
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
