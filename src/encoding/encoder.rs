use super::{error::Error, utils::Pid, FixedHeader, QoS};

#[cfg(feature = "mqttv5")]
use crate::{Properties, Property, PropertyIdentifier};

pub struct MqttEncoder<'a> {
    buf: &'a mut [u8],
    offset: usize,
    include_header: bool,
}

impl<'a> MqttEncoder<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            offset: 0,
            include_header: true,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.offset]
    }

    pub(crate) fn write_fixed_header(&mut self, v: &impl FixedHeader) -> Result<(), Error> {
        if self.include_header {
            self.write_u8(v.packet_type() | v.flags())?;
            self.write_varint(v.remaining_len() as u32)?;
        } else {
            self.check_remaining(v.remaining_len() as u32)?;
        }

        Ok(())
    }

    pub(crate) fn check_remaining(&self, len: u32) -> Result<(), Error> {
        if (self.buf[self.offset..].len() as u32) < len {
            debug!(
                "Buffer too small! Buffer len: {}, remaining: {}, requested: {}",
                self.buf.len(),
                self.buf[self.offset..].len(),
                len
            );
            Err(Error::BufferSize)
        } else {
            Ok(())
        }
    }

    pub(crate) fn write_varint(&mut self, len: u32) -> Result<(), Error> {
        self.check_remaining(len)?;

        let mut x = len;
        for _ in 0..4 {
            let mut byte = (x % 128) as u8;
            x /= 128;
            if x > 0 {
                byte |= 128;
            }
            self.write_u8(byte)?;
            if x == 0 {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn write_str(&mut self, v: &str) -> Result<(), Error> {
        self.write_slice(v.as_bytes())?;
        Ok(())
    }

    pub(crate) fn write_u8(&mut self, v: u8) -> Result<(), Error> {
        self.buf[self.offset] = v;
        self.offset += 1;
        Ok(())
    }

    pub(crate) fn write_u16(&mut self, v: u16) -> Result<(), Error> {
        self.write_payload(&v.to_be_bytes())?;
        Ok(())
    }

    pub(crate) fn write_slice(&mut self, v: &[u8]) -> Result<(), Error> {
        self.write_u16(v.len() as u16)?;
        self.write_payload(v)?;

        Ok(())
    }

    pub(crate) fn write_payload(&mut self, v: &[u8]) -> Result<(), Error> {
        self.check_remaining(v.len() as u32)?;
        self.buf[self.offset..self.offset + v.len()].copy_from_slice(v);
        self.offset += v.len();

        Ok(())
    }

    #[cfg(feature = "mqttv5")]
    pub(crate) fn write_properties(&mut self, v: &Properties<'_>) -> Result<(), Error> {
        self.write_varint(v.size() as u32)?;
        match v {
            Properties::Slice(properties) => {
                for property in *properties {
                    self.write_property(property)?;
                }
            }
            Properties::DataBlock(data) => self.write_slice(data)?,
            Properties::CorrelatedSlice {
                correlation,
                properties,
            } => {
                self.write_property(correlation)?;
                for property in *properties {
                    self.write_property(property)?;
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "mqttv5")]
    pub(crate) fn write_property(&mut self, v: &Property<'_>) -> Result<(), Error> {
        let identifier: PropertyIdentifier = v.into();
        self.write_varint(identifier as u32)?;

        match v {
            Property::PayloadFormatIndicator(v) => self.write_u8(*v)?,
            Property::MessageExpiryInterval(v) => self.write_varint(*v)?,
            Property::ContentType(v) => self.write_str(v)?,
            Property::ResponseTopic(v) => self.write_str(v)?,
            Property::CorrelationData(v) => self.write_slice(v)?,
            Property::SubscriptionIdentifier(v) => self.write_varint(*v)?,
            Property::SessionExpiryInterval(v) => self.write_varint(*v)?,
            Property::AssignedClientIdentifier(v) => self.write_str(v)?,
            Property::ServerKeepAlive(v) => self.write_u16(*v)?,
            Property::AuthenticationMethod(v) => self.write_str(v)?,
            Property::AuthenticationData(v) => self.write_slice(v)?,
            Property::RequestProblemInformation(v) => self.write_u8(*v)?,
            Property::WillDelayInterval(v) => self.write_varint(*v)?,
            Property::RequestResponseInformation(v) => self.write_u8(*v)?,
            Property::ResponseInformation(v) => self.write_str(v)?,
            Property::ServerReference(v) => self.write_str(v)?,
            Property::ReasonString(v) => self.write_str(v)?,
            Property::ReceiveMaximum(v) => self.write_u16(*v)?,
            Property::TopicAliasMaximum(v) => self.write_u16(*v)?,
            Property::TopicAlias(v) => self.write_u16(*v)?,
            Property::MaximumQoS(v) => self.write_u8(*v)?,
            Property::RetainAvailable(v) => self.write_u8(*v)?,
            Property::UserProperty(k, v) => {
                self.write_str(k)?;
                self.write_str(v)?
            }
            Property::MaximumPacketSize(v) => self.write_varint(*v)?,
            Property::WildcardSubscriptionAvailable(v) => self.write_u8(*v)?,
            Property::SubscriptionIdentifierAvailable(v) => self.write_u8(*v)?,
            Property::SharedSubscriptionAvailable(v) => self.write_u8(*v)?,
        }
        Ok(())
    }
}

#[allow(unused_variables)]
pub trait MqttEncode: FixedHeader {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error>;

    fn set_pid(&mut self, pid: Pid) {}

    fn get_qos(&self) -> Option<QoS> {
        None
    }
}
