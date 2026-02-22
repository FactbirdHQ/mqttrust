use num_enum::{FromPrimitive, IntoPrimitive};

/// MQTTv5-defined codes that may be returned in response to control packets.
#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum ConnAckReasonCode {
    /// Connection accepted.
    Success = 0x00,

    /// Unspecified error.
    UnspecifiedError = 0x80,
    /// Malformed packet.
    MalformedPacket = 0x81,
    /// Protocol error.
    ProtocolError = 0x82,
    /// Implementation-specific error.
    ImplementationError = 0x83,
    /// Unsupported protocol version.
    UnsupportedProtocol = 0x84,
    /// Client identifier not valid.
    ClientIdentifierInvalid = 0x85,
    /// Bad user name or password.
    BadUsernameOrPassword = 0x86,
    /// Not authorized.
    NotAuthorized = 0x87,
    /// Server unavailable.
    ServerUnavailable = 0x88,
    /// Server busy.
    ServerBusy = 0x89,
    /// Banned.
    Banned = 0x8a,
    /// Bad authentication method.
    BadAuthMethod = 0x8c,
    /// Topic name invalid.
    TopicNameInvalid = 0x90,
    /// Packet too large.
    PacketTooLarge = 0x95,
    /// Message rate too high.
    MessageRateTooHigh = 0x96,
    /// Quota exceeded.
    QuotaExceeded = 0x97,
    /// Payload format invalid.
    PayloadFormatInvalid = 0x99,
    /// Retain not supported.
    RetainNotSupported = 0x9A,
    /// QoS not supported.
    QosNotSupported = 0x9b,
    /// Use another server.
    UseAnotherServer = 0x9c,
    /// Server moved.
    ServerMoved = 0x9d,
    /// Connection rate exceeded.
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

/// Publish acknowledge (PUBACK) reason codes.
#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PubAckReasonCode {
    /// Success.
    Success = 0x00,
    /// No matching subscribers.
    NoMatchingSubscribers = 0x10,
    /// Unspecified error.
    UnspecifiedError = 0x80,
    /// Implementation-specific error.
    ImplementationError = 0x83,
    /// Not authorized.
    NotAuthorized = 0x87,
    /// Topic name invalid.
    TopicNameInvalid = 0x90,
    /// Packet identifier in use.
    PacketIdInUse = 0x91,
    /// Quota exceeded.
    QuotaExceeded = 0x97,
    /// Payload format invalid.
    PayloadFormatInvalid = 0x99,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

/// Publish received (PUBREC) reason codes.
#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PubRecReasonCode {
    /// Success.
    Success = 0x00,
    /// No matching subscribers.
    NoMatchingSubscribers = 0x10,
    /// Unspecified error.
    UnspecifiedError = 0x80,
    /// Implementation-specific error.
    ImplementationError = 0x83,
    /// Not authorized.
    NotAuthorized = 0x87,
    /// Topic name invalid.
    TopicNameInvalid = 0x90,
    /// Packet identifier in use.
    PacketIdInUse = 0x91,
    /// Quota exceeded.
    QuotaExceeded = 0x97,
    /// Payload format invalid.
    PayloadFormatInvalid = 0x99,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

/// Publish release (PUBREL) reason codes.
#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PubRelReasonCode {
    /// Success.
    Success = 0x00,
    /// Packet identifier not found.
    PacketIdNotFound = 0x92,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

/// Publish complete (PUBCOMP) reason codes.
#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PubCompReasonCode {
    /// Success.
    Success = 0x00,
    /// Packet identifier not found.
    PacketIdNotFound = 0x92,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

/// Subscribe acknowledgement reason codes for MQTTv3.
///
/// [SubAck] packets contain a `Vec` of these.
///
/// [SubAck]: struct.SubAck.html
#[cfg(feature = "mqttv3")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum SubAckReasonCode {
    /// Success - Maximum QoS 0.
    SuccessMaxQoS0 = 0x00,
    /// Success - Maximum QoS 1.
    SuccessMaxQoS1 = 0x01,
    /// Success - Maximum QoS 2.
    SuccessMaxQoS2 = 0x02,
    /// Failure.
    Failure = 0x80,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

#[cfg(feature = "mqttv3")]
impl SubAckReasonCode {
    pub fn success(&self) -> bool {
        let value = u8::from(*self);
        value < 0x80
    }
}

/// Subscribe acknowledgement reason codes for MQTTv5.
#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum SubAckReasonCode {
    /// Granted QoS 0.
    GrantedQoS0 = 0x00,
    /// Granted QoS 1.
    GrantedQoS1 = 0x01,
    /// Granted QoS 2.
    GrantedQoS2 = 0x02,
    /// Unspecified error.
    UnspecifiedError = 0x80,
    /// Implementation-specific error.
    ImplementationSpecificError = 0x83,
    /// Not authorized.
    NotAuthorized = 0x87,
    /// Topic filter invalid.
    TopicFilterInvalid = 0x8F,
    /// Packet identifier in use.
    PacketIdentifierInUse = 0x91,
    /// Quota exceeded.
    QuotaExceeded = 0x97,
    /// Shared subscriptions not supported.
    SharedSubscriptionsNotSupported = 0x9E,
    /// Subscription identifiers not supported.
    SubscriptionIdentifiersNotSupported = 0xA1,
    /// Wildcard subscriptions not supported.
    WildcardSubscriptionsNotSupported = 0xA2,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

#[cfg(feature = "mqttv5")]
impl SubAckReasonCode {
    pub fn success(&self) -> bool {
        let value = u8::from(*self);
        value < 0x80
    }
}

/// Unsubscribe acknowledgement reason codes for MQTTv5.
#[cfg(feature = "mqttv5")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum UnsubAckReasonCode {
    /// Success.
    Success = 0x00,
    /// No subscription existed.
    NoSubscriptionExisted = 0x11,
    /// Unspecified error.
    UnspecifiedError = 0x80,
    /// Implementation-specific error.
    ImplementationSpecificError = 0x83,
    /// Not authorized.
    NotAuthorized = 0x87,
    /// Topic filter invalid.
    TopicFilterInvalid = 0x8F,
    /// Packet identifier in use.
    PacketIdentifierInUse = 0x91,

    /// The reason code is not one of the documented MQTT reason codes.
    #[num_enum(default)]
    Unknown = 0xFF,
}

#[cfg(feature = "mqttv5")]
impl UnsubAckReasonCode {
    pub fn success(&self) -> bool {
        let value = u8::from(*self);
        value < 0x80
    }
}

/// Connect Return code.
///
/// [ConnAck]: struct.Connect.html
#[cfg(feature = "mqttv3")]
#[derive(PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum ConnAckReasonCode {
    /// Connection accepted.
    ConnectionAccepted = 0x00,

    /// Unacceptable protocol version.
    UnacceptableProtocolVersion = 0x01,
    /// Identifier rejected.
    IdentifierRejected = 0x02,
    /// Server unavailable.
    ServerUnavailable = 0x03,
    /// Bad user name or password.
    BadUsernameOrPassword = 0x04,
    /// Not authorized.
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

/// Disconnect reason codes for MQTTv3.
#[cfg(feature = "mqttv3")]
#[derive(Default, PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum DisconnectReasonCode {
    /// Normal disconnection.
    #[default]
    Normal = 0x00,
}

/// MQTTv5-defined codes that may be sent as Disconnect Reason Code values.
#[cfg(feature = "mqttv5")]
#[derive(Default, PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum DisconnectReasonCode {
    /// Normal disconnection.
    #[default]
    Normal = 0x00,
    /// The Client wishes to disconnect but requires that the Server also publishes its Will Message.
    DisconnectWithWillMessage = 0x04,
    /// The Connection is closed but the sender either does not wish to reveal the reason, or none of the other Reason Codes apply.
    UnspecifiedError = 0x80,
    /// Malformed packet.
    MalformedPacket = 0x81,
    /// Protocol error.
    ProtocolError = 0x82,
    /// Implementation-specific error.
    ImplementationSpecificError = 0x83,
    /// Not authorized.
    NotAuthorized = 0x87,
    /// Server busy.
    ServerBusy = 0x89,
    /// Server shutting down.
    ServerShuttingDown = 0x8B,
    /// Keep alive timeout.
    KeepAliveTimeout = 0x8D,
    /// Session taken over.
    SessionTakenOver = 0x8E,
    /// Topic filter invalid.
    TopicFilterInvalid = 0x8F,
    /// Topic name invalid.
    TopicNameInvalid = 0x90,
    /// Receive maximum exceeded.
    ReceiveMaximumExceeded = 0x93,
    /// Topic alias invalid.
    TopicAliasInvalid = 0x94,
    /// Packet too large.
    PacketTooLarge = 0x95,
    /// Message rate too high.
    MessageRateTooHigh = 0x96,
    /// Quota exceeded.
    QuotaExceeded = 0x97,
    /// Administrative action.
    AdministrativeAction = 0x98,
    /// Payload format invalid.
    PayloadFormatInvalid = 0x99,
    /// Retain not supported.
    RetainNotSupported = 0x9A,
    /// QoS not supported.
    QoSNotSupported = 0x9B,
    /// Use another server.
    UseAnotherServer = 0x9C,
    /// Server moved.
    ServerMoved = 0x9D,
    /// Shared subscriptions not supported.
    SharedSubscriptionsNotSupported = 0x9E,
    /// Connection rate exceeded.
    ConnectionRateExceeded = 0x9F,
    /// Maximum connect time exceeded.
    MaximumConnectTime = 0xA0,
    /// Subscription identifiers not supported.
    SubscriptionIdentifiersNotSupported = 0xA1,
    /// Wildcard subscriptions not supported.
    WildcardSubscriptionsNotSupported = 0xA2,
}
