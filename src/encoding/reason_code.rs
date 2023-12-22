use num_enum::{FromPrimitive, IntoPrimitive};

/// MQTTv5-defined codes that may be returned in response to control packets.
#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum ConnAckReasonCode {
    Success = 0x00,

    UnspecifiedError = 0x80,
    MalformedPacket = 0x81,
    ProtocolError = 0x82,
    ImplementationError = 0x83,
    UnsupportedProtocol = 0x84,
    ClientIdentifierInvalid = 0x85,
    BadUsernameOrPassword = 0x86,
    NotAuthorized = 0x87,
    ServerUnavailable = 0x88,
    ServerBusy = 0x89,
    Banned = 0x8a,
    BadAuthMethod = 0x8c,
    TopicNameInvalid = 0x90,
    PacketTooLarge = 0x95,
    MessageRateTooHigh = 0x96,
    QuotaExceeded = 0x97,
    PayloadFormatInvalid = 0x99,
    RetainNotSupported = 0x9A,
    QosNotSupported = 0x9b,
    UseAnotherServer = 0x9c,
    ServerMoved = 0x9d,
    ConnectionRateExceeded = 0x9f,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

#[cfg(feature = "mqttv5")]
impl ConnAckReasonCode {
    pub fn success(&self) -> bool {
        let value = u8::from(*self);
        value < 0x80
    }
}

#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PubAckReasonCode {
    Success = 0x00,
    NoMatchingSubscribers = 0x10,
    UnspecifiedError = 0x80,
    ImplementationError = 0x83,
    NotAuthorized = 0x87,
    TopicNameInvalid = 0x90,
    PacketIdInUse = 0x91,
    QuotaExceeded = 0x97,
    PayloadFormatInvalid = 0x99,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PubRecReasonCode {
    Success = 0x00,
    NoMatchingSubscribers = 0x10,
    UnspecifiedError = 0x80,
    ImplementationError = 0x83,
    NotAuthorized = 0x87,
    TopicNameInvalid = 0x90,
    PacketIdInUse = 0x91,
    QuotaExceeded = 0x97,
    PayloadFormatInvalid = 0x99,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PubRelReasonCode {
    Success = 0x00,
    PacketIdNotFound = 0x92,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PubCompReasonCode {
    Success = 0x00,
    PacketIdNotFound = 0x92,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

/// Subscribe return value.
///
/// [SubAck] packets contain a `Vec` of those.
///
/// [SubAck]: struct.Subscribe.html
#[cfg(feature = "mqttv3")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum SubAckReturnCode {
    SuccessMaxQoS0 = 0x00,
    SuccessMaxQoS1 = 0x01,
    SuccessMaxQoS2 = 0x02,
    Failure = 0x80,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

/// Connect Return code.
///
/// [ConnAck]: struct.Connect.html
#[cfg(feature = "mqttv3")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum ConnAckReasonCode {
    ConnectionAccepted = 0x00,

    UnacceptableProtocolVersion = 0x01,
    IdentifierRejected = 0x02,
    ServerUnavailable = 0x03,
    BadUsernameOrPassword = 0x04,
    NotAuthorized = 0x05,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

#[cfg(feature = "mqttv3")]
impl ConnAckReasonCode {
    pub fn success(&self) -> bool {
        matches!(self, Self::ConnectionAccepted)
    }
}
