use crate::{
    encoding::{self, utils::StateError, ConnectReturnCode},
    reason_codes::ReasonCode,
};
// use crate::de::Error as DeError;
// use crate::ser::Error as SerError;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    Encoding(encoding::Error),
}

impl From<crate::encoding::Error> for Error {
    fn from(value: encoding::Error) -> Self {
        Self::Encoding(value)
    }
}

/// Errors that are specific to the MQTT protocol implementation.
#[non_exhaustive]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ProtocolError {
    ProvidedClientIdTooLong,
    UnexpectedPacket,
    InvalidProperty,
    MalformedPacket,
    BufferSize,
    BadIdentifier,
    Unacknowledged,
    WrongQos,
    UnsupportedPacket,
    NoTopic,
    AuthAlreadySpecified,
    WillAlreadySpecified,
    Failed(ReasonCode),
    // Serialization(SerError),
    // Deserialization(DeError),
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionError {
    MqttState(StateError),
    NetworkTimeout,
    FlushTimeout,
    Io(embedded_io_async::ErrorKind),
    ConnectionRefused(ConnectReturnCode),
    NotConnAck,
    RequestsDone,
}
