use crate::encoding::{self, StateError};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    /// Unexpected end of input while parsing a packet.
    EOF,
    /// Internal state machine received an unexpected transition.
    StateMismatch,
    /// Received a packet that violates the MQTT specification.
    MalformedPacket,
    /// Transport read operation failed.
    Read,
    /// Transport write operation failed.
    Write,
    /// Maximum number of concurrent subscribers exceeded.
    TooManyClients,
    /// The provided topic filter string is invalid.
    BadTopicFilter,
    /// Maximum number of in-flight messages reached.
    MaxInflight,
    /// An operation timed out.
    Timeout,
    /// A buffer or collection capacity was exceeded.
    Overflow,
    /// Error during packet encoding or decoding.
    Encoding(encoding::EncodingError),
}

impl From<crate::encoding::EncodingError> for Error {
    fn from(value: encoding::EncodingError) -> Self {
        Self::Encoding(value)
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ConnectionError {
    /// The MQTT state machine encountered an error.
    MqttState(StateError),
    /// Timed out waiting for a network connection.
    NetworkTimeout,
    /// Timed out flushing data to the transport.
    FlushTimeout,
    /// The broker address could not be resolved.
    InvalidAddress,
    /// Underlying I/O transport error.
    Io(embedded_io_async::ErrorKind),
    /// The broker refused the connection (non-success reason code).
    ConnectionRefused,
    /// Expected a CONNACK packet but received something else.
    NotConnAck,
    /// All pending requests have been processed.
    RequestsDone,
}
