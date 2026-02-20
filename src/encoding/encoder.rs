use super::{error::Error, utils::Pid, FixedHeader, QoS};

use crate::{varint_len, ToPayload};
#[cfg(feature = "mqttv5")]
use crate::{Properties, Property, PropertyIdentifier};

/// Maximum length of the MQTT header in bytes.
pub(crate) const MAX_MQTT_HEADER_LEN: usize = 5;

/// A struct that implements the MQTT encoder.
pub struct MqttEncoder<'a> {
    /// The buffer where the encoded packet will be written.
    buf: &'a mut [u8],
    /// The current offset in the buffer.
    offset: usize,
    /// The starting position of the header in the buffer.
    header_start: usize,
}

impl<'a> MqttEncoder<'a> {
    /// Creates a new `MqttEncoder` with the given buffer.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            offset: MAX_MQTT_HEADER_LEN,
            header_start: MAX_MQTT_HEADER_LEN,
        }
    }

    /// Returns a slice of the buffer containing the encoded packet.
    pub fn packet_bytes(&self) -> &[u8] {
        &self.buf[self.header_start..self.offset]
    }

    /// Consumes the encoder, returning the encoded packet as a slice of the
    /// original buffer with the buffer's lifetime.
    pub fn into_packet_bytes(self) -> &'a [u8] {
        let start = self.header_start;
        let end = self.offset;
        &self.buf[start..end]
    }

    /// Returns the number of bytes used in the buffer.
    pub fn used_size(&self) -> usize {
        self.offset
    }

    /// Finalizes the fixed header of the packet by writing the remaining length and packet type.
    ///
    /// This function must be called after all other data has been encoded.
    pub(crate) fn finalize_fixed_header(&mut self, v: &impl FixedHeader) -> Result<(), Error> {
        let remaining_len = self.offset - MAX_MQTT_HEADER_LEN;
        self.offset = self.header_start - varint_len(remaining_len) - 1;
        self.header_start = self.offset;
        self.write_u8(v.packet_type() | v.flags())?;
        self.write_varint(remaining_len as u32)?;
        self.offset += remaining_len;

        self.buf[..self.offset].rotate_left(self.header_start);
        self.offset -= self.header_start;
        self.header_start = 0;

        Ok(())
    }

    /// Checks if there is enough space in the buffer to write the specified number of bytes.
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

    /// Encodes a variable length integer.
    pub(crate) fn write_varint(&mut self, mut len: u32) -> Result<(), Error> {
        for _ in 0..4 {
            let mut byte = (len % 128) as u8;
            len /= 128;
            if len > 0 {
                byte |= 128;
            }
            self.write_u8(byte)?;
            if len == 0 {
                break;
            }
        }
        Ok(())
    }

    /// Encodes a string into the buffer.
    pub(crate) fn write_str(&mut self, v: &str) -> Result<(), Error> {
        self.write_slice(v.as_bytes())?;
        Ok(())
    }

    /// Encodes a single byte into the buffer.
    pub(crate) fn write_u8(&mut self, v: u8) -> Result<(), Error> {
        self.buf[self.offset] = v;
        self.offset += 1;
        Ok(())
    }

    /// Encodes a 16-bit unsigned integer into the buffer.
    pub(crate) fn write_u16(&mut self, v: u16) -> Result<(), Error> {
        self.check_remaining(2)?;
        self.buf[self.offset..self.offset + 2].copy_from_slice(&v.to_be_bytes());
        self.offset += 2;
        Ok(())
    }

    /// Encodes a 32-bit unsigned integer into the buffer.
    pub(crate) fn write_u32(&mut self, v: u32) -> Result<(), Error> {
        self.check_remaining(4)?;
        self.buf[self.offset..self.offset + 4].copy_from_slice(&v.to_be_bytes());
        self.offset += 4;
        Ok(())
    }

    /// Encodes a byte slice into the buffer.
    pub(crate) fn write_slice(&mut self, v: &[u8]) -> Result<(), Error> {
        self.check_remaining(2 + v.len() as u32)?;

        // Write a u16 length first as big-endian
        self.buf[self.offset..self.offset + 2].copy_from_slice(&(v.len() as u16).to_be_bytes());
        self.offset += 2;

        // Write the payload slice
        self.buf[self.offset..self.offset + v.len()].copy_from_slice(v);
        self.offset += v.len();

        Ok(())
    }

    /// Encodes a payload into the buffer.
    pub(crate) fn write_payload<P: ToPayload>(&mut self, payload: &P) -> Result<(), Error> {
        self.offset += payload.serialize(&mut self.buf[self.offset..])?;

        Ok(())
    }

    /// Encodes MQTTv5 properties into the buffer.
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

    /// Encodes a single MQTTv5 property into the buffer.
    #[cfg(feature = "mqttv5")]
    pub(crate) fn write_property(&mut self, v: &Property<'_>) -> Result<(), Error> {
        let identifier: PropertyIdentifier = v.into();
        self.write_u8(identifier as u8)?;

        match v {
            Property::PayloadFormatIndicator(v) => self.write_u8(*v)?,
            Property::MessageExpiryInterval(v) => self.write_u32(*v)?,
            Property::ContentType(v) => self.write_str(v)?,
            Property::ResponseTopic(v) => self.write_str(v)?,
            Property::CorrelationData(v) => self.write_slice(v)?,
            Property::SubscriptionIdentifier(v) => self.write_varint(*v)?,
            Property::SessionExpiryInterval(v) => self.write_u32(*v)?,
            Property::AssignedClientIdentifier(v) => self.write_str(v)?,
            Property::ServerKeepAlive(v) => self.write_u16(*v)?,
            Property::AuthenticationMethod(v) => self.write_str(v)?,
            Property::AuthenticationData(v) => self.write_slice(v)?,
            Property::RequestProblemInformation(v) => self.write_u8(*v)?,
            Property::WillDelayInterval(v) => self.write_u32(*v)?,
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
            Property::MaximumPacketSize(v) => self.write_u32(*v)?,
            Property::WildcardSubscriptionAvailable(v) => self.write_u8(*v)?,
            Property::SubscriptionIdentifierAvailable(v) => self.write_u8(*v)?,
            Property::SharedSubscriptionAvailable(v) => self.write_u8(*v)?,
        }
        Ok(())
    }
}

/// A trait for encoding MQTT packets into a buffer.
pub trait MqttEncode: FixedHeader {
    /// Encodes the packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error>;

    /// Returns the maximum packet size in bytes.
    fn max_packet_size(&self) -> usize;

    /// Sets the packet identifier for the packet.
    ///
    /// This is only used for packets that have a packet identifier.
    #[allow(unused_variables)]
    fn set_pid(&mut self, pid: Pid) {}

    /// Returns the quality of service (QoS) of the packet.
    ///
    /// This is only used for packets that have a QoS.
    fn get_qos(&self) -> Option<QoS> {
        None
    }
}
