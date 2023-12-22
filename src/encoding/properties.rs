/// All of the possible properties that MQTT version 5 supports.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Property<'a> {
    PayloadFormatIndicator(u8),
    MessageExpiryInterval(u32),
    ContentType(&'a str),
    ResponseTopic(&'a str),
    CorrelationData(&'a [u8]),
    SubscriptionIdentifier(u32),
    SessionExpiryInterval(u32),
    AssignedClientIdentifier(&'a str),
    ServerKeepAlive(u16),
    AuthenticationMethod(&'a str),
    AuthenticationData(&'a [u8]),
    RequestProblemInformation(u8),
    WillDelayInterval(u32),
    RequestResponseInformation(u8),
    ResponseInformation(&'a str),
    ServerReference(&'a str),
    ReasonString(&'a str),
    ReceiveMaximum(u16),
    TopicAliasMaximum(u16),
    TopicAlias(u16),
    MaximumQoS(u8),
    RetainAvailable(u8),
    UserProperty(&'a str, &'a str),
    MaximumPacketSize(u32),
    WildcardSubscriptionAvailable(u8),
    SubscriptionIdentifierAvailable(u8),
    SharedSubscriptionAvailable(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Properties<'a> {
    /// Properties ready for transmission are provided as a list of properties that will be later
    /// encoded into a packet.
    Slice(&'a [Property<'a>]),

    /// Properties have an unknown size when being received. As such, we store them as a binary
    /// blob that we iterate across.
    DataBlock(&'a [u8]),

    /// Properties that are correlated to a previous message.
    CorrelatedSlice {
        correlation: Property<'a>,
        properties: &'a [Property<'a>],
    },
}
