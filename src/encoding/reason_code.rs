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

#[cfg(feature = "mqttv3")]
#[derive(Default, PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum DisconnectReasonCode {
    #[default]
    /// Normal disconnection
    Normal = 0x00,
}

/// MQTTv5-defined codes that may be sent as Disconnect Reason Code values.
#[cfg(feature = "mqttv5")]
#[derive(Default, PartialEq, PartialOrd, Copy, Clone, Debug, FromPrimitive, IntoPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum DisconnectReasonCode {
    #[default]
    /// Normal disconnection
    Normal = 0x00,
    /// The Client wishes to disconnect but requires that the Server also publishes its Will Message.
    DisconnectWithWillMessage = 0x04,
    /// The Connection is closed but the sender either does not wish to reveal the reason, or none of the other Reason Codes apply.
    UnspecifiedError = 0x80,
    /// The received packet does not conform to this specification.
    MalformedPacket = 0x81,
    /// An unexpected or out of order packet was received.
    ProtocolError = 0x82,
    /// The packet received is valid but cannot be processed by this implementation.
    ImplementationSpecificError = 0x83,
    /// The request is not authorized.
    NotAuthorized = 0x87,
    /// The Server is busy and cannot continue processing requests from this Client.
    ServerBusy = 0x89,
    /// The Server is shutting down.
    ServerShuttingDown = 0x8B,
    /// The Connection is closed because no packet has been received for 1.5 times the Keepalive time.
    KeepAliveTimeout = 0x8D,
    /// Another Connection using the same ClientID has connected causing this Connection to be closed.
    SessionTakenOver = 0x8E,
    /// The Topic Filter is correctly formed, but is not accepted by this Sever.
    TopicFilterInvalid = 0x8F,
    /// The Topic Name is correctly formed, but is not accepted by this Client or Server.
    TopicNameInvalid = 0x90,
    /// The Client or Server has received more than Receive Maximum publication for which it has not sent PUBACK or PUBCOMP.
    ReceiveMaximumExceeded = 0x93,
    /// The Client or Server has received a PUBLISH packet containing a Topic Alias which is greater than the Maximum Topic Alias it sent in the CONNECT or CONNACK packet.
    TopicAliasInvalid = 0x94,
    /// The packet size is greater than Maximum Packet Size for this Client or Server.
    PacketTooLarge = 0x95,
    /// The received data rate is too high.
    MessageRateTooHigh = 0x96,
    /// An implementation or administrative imposed limit has been exceeded.
    QuotaExceeded = 0x97,
    /// The Connection is closed due to an administrative action.
    AdministrativeAction = 0x98,
    /// The payload format does not match the one specified by the Payload Format Indicator.
    PayloadFormatInvalid = 0x99,
    /// The Server has does not support retained messages.
    RetainNotSupported = 0x9A,
    /// The Client specified a QoS greater than the QoS specified in a Maximum QoS in the CONNACK.
    QoSNotSupported = 0x9B,
    /// The Client should temporarily change its Server.
    UseAnotherServer = 0x9C,
    /// The Server is moved and the Client should permanently change its server location.
    ServerMoved = 0x9D,
    /// The Server does not support Shared Subscriptions.
    SharedSubscriptionsNotSupported = 0x9E,
    /// This connection is closed because the connection rate is too high.
    ConnectionRateExceeded = 0x9F,
    /// The maximum connection time authorized for this connection has been exceeded.
    MaximumConnectTime = 0xA0,
    /// The Server does not support Subscription Identifiers; the subscription is not accepted.
    SubscriptionIdentifiersNotSupported = 0xA1,
    /// The Server does not support Wildcard Subscriptions; the subscription is not accepted.
    WildcardSubscriptionsNotSupported = 0xA2,
}
