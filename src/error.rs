use crate::reason_codes::ReasonCode;
use crate::de::Error as DeError;
use crate::ser::Error as SerError;

pub enum Error {
    EOF,
    StateMismatch,
    MalformedPacket,
    Read,
    TooManyClients
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
    Serialization(SerError),
    Deserialization(DeError),
}