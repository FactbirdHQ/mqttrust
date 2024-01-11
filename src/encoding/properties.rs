use num_enum::TryFromPrimitive;

use crate::{varint_len, Error};

#[derive(Debug, Copy, Clone, PartialEq, TryFromPrimitive)]
#[repr(u32)]
pub(crate) enum PropertyIdentifier {
    Invalid = u32::MAX,

    PayloadFormatIndicator = 0x01,
    MessageExpiryInterval = 0x02,
    ContentType = 0x03,

    ResponseTopic = 0x08,
    CorrelationData = 0x09,

    SubscriptionIdentifier = 0x0B,

    SessionExpiryInterval = 0x11,
    AssignedClientIdentifier = 0x12,
    ServerKeepAlive = 0x13,
    AuthenticationMethod = 0x15,
    AuthenticationData = 0x16,
    RequestProblemInformation = 0x17,
    WillDelayInterval = 0x18,
    RequestResponseInformation = 0x19,

    ResponseInformation = 0x1A,

    ServerReference = 0x1C,

    ReasonString = 0x1F,

    ReceiveMaximum = 0x21,
    TopicAliasMaximum = 0x22,
    TopicAlias = 0x23,
    MaximumQoS = 0x24,
    RetainAvailable = 0x25,
    UserProperty = 0x26,
    MaximumPacketSize = 0x27,
    WildcardSubscriptionAvailable = 0x28,
    SubscriptionIdentifierAvailable = 0x29,
    SharedSubscriptionAvailable = 0x2A,
}

/// All of the possible properties that MQTT version 5 supports.
#[derive(Debug, Copy, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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

impl<'a> Property<'a> {
    fn size(&self) -> usize {
        let identifier: PropertyIdentifier = self.into();
        let identifier_length = varint_len(identifier as usize);

        match self {
            Property::ContentType(data)
            | Property::ResponseTopic(data)
            | Property::AuthenticationMethod(data)
            | Property::ResponseInformation(data)
            | Property::ServerReference(data)
            | Property::ReasonString(data)
            | Property::AssignedClientIdentifier(data) => data.len() + 2 + identifier_length,

            Property::UserProperty(key, value) => {
                (value.len() + 2) + (key.len() + 2) + identifier_length
            }

            Property::CorrelationData(data) | Property::AuthenticationData(data) => {
                data.len() + 2 + identifier_length
            }

            Property::SubscriptionIdentifier(id) => varint_len(*id as usize) + identifier_length,

            Property::MessageExpiryInterval(_)
            | Property::SessionExpiryInterval(_)
            | Property::WillDelayInterval(_)
            | Property::MaximumPacketSize(_) => 4 + identifier_length,

            Property::ServerKeepAlive(_)
            | Property::ReceiveMaximum(_)
            | Property::TopicAliasMaximum(_)
            | Property::TopicAlias(_) => 2 + identifier_length,

            Property::PayloadFormatIndicator(_)
            | Property::RequestProblemInformation(_)
            | Property::RequestResponseInformation(_)
            | Property::MaximumQoS(_)
            | Property::RetainAvailable(_)
            | Property::WildcardSubscriptionAvailable(_)
            | Property::SubscriptionIdentifierAvailable(_)
            | Property::SharedSubscriptionAvailable(_) => 1 + identifier_length,
        }
    }
}

impl<'a> From<&Property<'a>> for PropertyIdentifier {
    fn from(prop: &Property<'a>) -> PropertyIdentifier {
        match prop {
            Property::PayloadFormatIndicator(_) => PropertyIdentifier::PayloadFormatIndicator,
            Property::MessageExpiryInterval(_) => PropertyIdentifier::MessageExpiryInterval,
            Property::ContentType(_) => PropertyIdentifier::ContentType,
            Property::ResponseTopic(_) => PropertyIdentifier::ResponseTopic,
            Property::CorrelationData(_) => PropertyIdentifier::CorrelationData,
            Property::SubscriptionIdentifier(_) => PropertyIdentifier::SubscriptionIdentifier,
            Property::SessionExpiryInterval(_) => PropertyIdentifier::SessionExpiryInterval,
            Property::AssignedClientIdentifier(_) => PropertyIdentifier::AssignedClientIdentifier,
            Property::ServerKeepAlive(_) => PropertyIdentifier::ServerKeepAlive,
            Property::AuthenticationMethod(_) => PropertyIdentifier::AuthenticationMethod,
            Property::AuthenticationData(_) => PropertyIdentifier::AuthenticationData,
            Property::RequestProblemInformation(_) => PropertyIdentifier::RequestProblemInformation,
            Property::WillDelayInterval(_) => PropertyIdentifier::WillDelayInterval,
            Property::RequestResponseInformation(_) => {
                PropertyIdentifier::RequestResponseInformation
            }
            Property::ResponseInformation(_) => PropertyIdentifier::ResponseInformation,
            Property::ServerReference(_) => PropertyIdentifier::ServerReference,
            Property::ReasonString(_) => PropertyIdentifier::ReasonString,
            Property::ReceiveMaximum(_) => PropertyIdentifier::ReceiveMaximum,
            Property::TopicAliasMaximum(_) => PropertyIdentifier::TopicAliasMaximum,
            Property::TopicAlias(_) => PropertyIdentifier::TopicAlias,
            Property::MaximumQoS(_) => PropertyIdentifier::MaximumQoS,
            Property::RetainAvailable(_) => PropertyIdentifier::RetainAvailable,
            Property::UserProperty(_, _) => PropertyIdentifier::UserProperty,
            Property::MaximumPacketSize(_) => PropertyIdentifier::MaximumPacketSize,
            Property::WildcardSubscriptionAvailable(_) => {
                PropertyIdentifier::WildcardSubscriptionAvailable
            }
            Property::SubscriptionIdentifierAvailable(_) => {
                PropertyIdentifier::SubscriptionIdentifierAvailable
            }
            Property::SharedSubscriptionAvailable(_) => {
                PropertyIdentifier::SharedSubscriptionAvailable
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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

impl<'a> Properties<'a> {
    /// The length in bytes of the serialized properties.
    pub fn size(&self) -> usize {
        // Properties in MQTTv5 must be prefixed with a variable-length integer denoting the size
        // of the all of the properties in bytes.
        match self {
            Properties::Slice(props) => props.iter().map(|prop| prop.size()).sum(),
            Properties::CorrelatedSlice {
                correlation,
                properties,
            } => properties
                .iter()
                .chain([*correlation].iter())
                .map(|prop| prop.size())
                .sum(),
            Properties::DataBlock(block) => block.len(),
        }
    }
}

/// Used to progressively iterate across binary property blocks, deserializing them along the way.
pub struct PropertiesIter<'a> {
    props: &'a [u8],
    index: usize,
}

impl<'a> PropertiesIter<'a> {
    pub fn response_topic(&mut self) -> Option<&'a str> {
        self.find_map(|prop| {
            if let Ok(crate::Property::ResponseTopic(topic)) = prop {
                Some(topic)
            } else {
                None
            }
        })
    }
}
impl<'a> core::iter::Iterator for PropertiesIter<'a> {
    type Item = Result<Property<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.props.len() {
            return None;
        }

        // TODO:
        // Progressively deserialize properties and yield them.
        // let mut deserializer = MqttDeserializer::new(&self.props[self.index..]);
        // let property =
        //     Property::deserialize(&mut deserializer).map_err(ProtocolError::Deserialization);
        // self.index += deserializer.deserialized_bytes();
        // Some(property)
        None
    }
}

impl<'a> core::iter::IntoIterator for &'a Properties<'a> {
    type Item = Result<Property<'a>, Error>;
    type IntoIter = PropertiesIter<'a>;

    fn into_iter(self) -> PropertiesIter<'a> {
        if let Properties::DataBlock(data) = self {
            PropertiesIter {
                props: data,
                index: 0,
            }
        } else {
            // Iterating over other property types is not implemented. The user may instead iterate
            // through slices directly.
            unimplemented!()
        }
    }
}
