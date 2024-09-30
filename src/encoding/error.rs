/// Errors returned by [`encode()`] and [`decode()`].
///
/// [`encode()`]: fn.encode.html
/// [`decode()`]: fn.decode.html
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    ProvidedClientIdTooLong,
    UnexpectedPacket,
    InsufficientBytes,
    InvalidProperty(u8),
    MalformedPacket,
    /// Not enough space in the write buffer.
    ///
    /// It is the caller's responsibility to pass a big enough buffer to `encode()`.
    BufferSize,
    BadIdentifier(u16),
    Unacknowledged,
    /// Tried to decode a QoS > 2.
    WrongQos(u8),
    UnsupportedPacket,
    NoTopic,
    AuthAlreadySpecified,
    WillAlreadySpecified,
    // Failed(ReasonCode),
    /// Tried to decode an unknown protocol.
    InvalidProtocol(heapless::String<10>, u8),
    /// Tried to decode an invalid fixed header (packet type, flags, or remaining_length).
    InvalidHeader,
    /// Trying to encode/decode an invalid length.
    ///
    /// The difference with `WriteZero`/`UnexpectedEof` is that it refers to an invalid/corrupt
    /// length rather than a buffer size issue.
    InvalidLength,
    /// Trying to decode a non-utf8 string.
    InvalidString,

    PidMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// Io Error while state is passed to network
    Io(embedded_io_async::ErrorKind),
    /// Invalid state for a given operation
    InvalidState,
    /// Received a packet (ack) which isn't asked for
    Unsolicited(u16),
    /// Last pingreq isn't acked
    AwaitPingResp,
    /// Received a wrong packet while waiting for another packet
    WrongPacket,
    CollisionTimeout,
    EmptySubscription,
    Deserialization,
    OutgoingPacketTooLarge {
        pkt_size: usize,
        max: usize,
    },
}

impl core::fmt::Display for StateError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Self::Io(kind) => write!(f, "IO error: {kind:?}"),
            Self::InvalidState => write!(f, "Invalid state for a given operation"),
            Self::Unsolicited(pid) => write!(f, "Received unsolicited ack pkid: {pid}"),
            Self::AwaitPingResp => write!(f, "Last pingreq isn't acked"),
            Self::WrongPacket => write!(f, "Received a wrong packet while waiting for another packet"),
            Self::CollisionTimeout => write!(f, "Timeout while waiting to resolve collision"),
            Self::EmptySubscription => write!(f, "A Subscribe packet must contain atleast one filter"),
            Self::Deserialization => write!(f, "Mqtt serialization/deserialization error"),
            Self::OutgoingPacketTooLarge { pkt_size, max} => write!(f, "Cannot receive packet of size '{pkt_size:?}'. It's greater than the client's maximum packet size of: '{max:?}'"),
        }
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for StateError {
    fn format(&self, f: defmt::Formatter) {
        match self {
            Self::Io(kind) => defmt::write!(f, "IO error: {:?}", kind),
            Self::InvalidState => defmt::write!(f, "Invalid state for a given operation"),
            Self::Unsolicited(pid) => defmt::write!(f, "Received unsolicited ack pkid: {}", pid),
            Self::AwaitPingResp => defmt::write!(f, "Last pingreq isn't acked"),
            Self::WrongPacket => defmt::write!(f, "Received a wrong packet while waiting for another packet"),
            Self::CollisionTimeout => defmt::write!(f, "Timeout while waiting to resolve collision"),
            Self::EmptySubscription => defmt::write!(f, "A Subscribe packet must contain atleast one filter"),
            Self::Deserialization => defmt::write!(f, "Mqtt serialization/deserialization error"),
            Self::OutgoingPacketTooLarge { pkt_size, max} => defmt::write!(f, "Cannot receive packet of size '{:?}'. It's greater than the client's maximum packet size of: '{:?}'", pkt_size, max),
        }
    }
}
