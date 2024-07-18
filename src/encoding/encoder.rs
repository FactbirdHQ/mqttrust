use super::{error::Error, utils::Pid, FixedHeader, QoS};

use crate::{varint_len, PacketType, ToPayload};
#[cfg(feature = "mqttv5")]
use crate::{Properties, Property, PropertyIdentifier};

pub(crate) const MAX_MQTT_HEADER_LEN: usize = 5;
pub(crate) const TX_HEADER_LEN: usize = 5;

pub struct MqttEncoder<'a> {
    buf: &'a mut [u8],
    offset: usize,
    header_start: usize,
    include_header: bool,
}

impl<'a> MqttEncoder<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            offset: MAX_MQTT_HEADER_LEN + TX_HEADER_LEN,
            header_start: MAX_MQTT_HEADER_LEN + TX_HEADER_LEN,
            include_header: true,
        }
    }

    pub fn packet_bytes(&self) -> &[u8] {
        &self.buf[self.header_start..self.offset]
    }

    pub fn used_size(&self) -> usize {
        self.offset
    }

    pub(crate) fn finalize_fixed_header(&mut self, v: &impl FixedHeader) -> Result<(), Error> {
        if self.include_header {
            let remaining_len = self.offset - MAX_MQTT_HEADER_LEN - TX_HEADER_LEN;
            self.offset = self.header_start - varint_len(remaining_len) - 1;
            self.header_start = self.offset;
            self.write_u8(v.packet_type() | v.flags())?;
            self.write_varint(remaining_len as u32)?;
            self.offset += remaining_len;
        }

        Ok(())
    }

    pub(crate) fn write_tx_header(
        &mut self,
        packet_type: PacketType,
        qos: Option<QoS>,
        pid: Option<Pid>,
    ) -> Result<TxHeader, Error> {
        let tx_header = TxHeader {
            typ: packet_type,
            qos,
            pid,
            packet_start: self.header_start as u8,
        };

        tx_header.to_bytes(&mut self.buf[0..TX_HEADER_LEN])?;

        Ok(tx_header)
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
        self.check_remaining(2)?;
        self.buf[self.offset..self.offset + 2].copy_from_slice(&v.to_be_bytes());
        self.offset += 2;
        Ok(())
    }

    pub(crate) fn write_u32(&mut self, v: u32) -> Result<(), Error> {
        self.check_remaining(4)?;
        self.buf[self.offset..self.offset + 4].copy_from_slice(&v.to_be_bytes());
        self.offset += 4;
        Ok(())
    }

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

    pub(crate) fn write_payload<P: ToPayload>(&mut self, payload: &P) -> Result<(), Error> {
        self.offset += payload.serialize(&mut self.buf[self.offset..])?;

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

#[allow(unused_variables)]
pub trait MqttEncode: FixedHeader {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<TxHeader, Error>;

    fn max_packet_size(&self) -> usize;

    fn set_pid(&mut self, pid: Pid) {}

    fn get_qos(&self) -> Option<QoS> {
        None
    }
}

// FIXME: don't hand-roll these serializations?
pub(crate) struct TxHeader {
    pub typ: PacketType,
    pub qos: Option<QoS>,
    pub pid: Option<Pid>,
    pub packet_start: u8,
}

impl TxHeader {
    pub(crate) fn from_bytes(bytes: &[u8]) -> (Self, &[u8]) {
        let packet_start = bytes[4];
        (
            Self {
                typ: PacketType::try_from(bytes[0]).unwrap(),
                qos: QoS::try_from(bytes[1]).ok(),
                pid: Pid::try_from(u16::from_le_bytes(bytes[2..4].try_into().unwrap())).ok(),
                packet_start,
            },
            &bytes[packet_start as usize..],
        )
    }

    pub(crate) fn to_bytes(&self, buf: &mut [u8]) -> Result<(), Error> {
        if buf.len() < 5 {
            return Err(Error::BufferSize);
        }

        buf[0] = u8::from(self.typ);
        buf[1] = match self.qos {
            Some(qos) => u8::from(qos),
            None => 0xFF,
        };
        buf[2..4].copy_from_slice(&self.pid.unwrap_or_default().get().to_le_bytes()[..]);
        buf[4] = self.packet_start;
        Ok(())
    }
}
