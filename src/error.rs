use crate::encoding::{self, StateError};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    EOF,
    StateMismatch,
    MalformedPacket,
    Read,
    Write,
    TooManyClients,
    BadTopicFilter,
    MaxInflight,
    Timeout,
    Overflow,
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
    MqttState(StateError),
    NetworkTimeout,
    FlushTimeout,
    InvalidAddress,
    Io(embedded_io_async::ErrorKind),
    ConnectionRefused,
    NotConnAck,
    RequestsDone,
}
