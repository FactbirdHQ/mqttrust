use super::{error::Error, packets::PacketType, utils::QoS};

#[cfg(feature = "mqttv5")]
use crate::Properties;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedHeader {
    pub typ: PacketType,
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
    pub remaining_len: usize,
}

pub struct MqttDecoder<'a> {
    buf: &'a [u8],
    offset: usize,
    packet_length: Option<usize>,
}

impl<'a> MqttDecoder<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            offset: 0,
            packet_length: None,
        }
    }

    pub fn read_fixed_header(&mut self) -> Result<FixedHeader, Error> {
        let hd = self.read_u8()?;
        let typ = PacketType::try_from(hd).map_err(|_| Error::InvalidHeader)?;
        let remaining_len = self.read_len()?;

        Ok(FixedHeader {
            typ,
            dup: hd & 0b1000 != 0,
            qos: QoS::try_from((hd & 0b110) >> 1).map_err(|_| Error::InvalidQos(hd & 0b110))?,
            retain: hd & 1 == 1,
            remaining_len,
        })
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    pub fn packet_len(&self) -> Option<usize> {
        // FIXME:
        self.packet_length
    }

    pub fn check_remaining(&self, len: usize) -> Result<(), Error> {
        if self.buf[self.offset..].len() < len {
            return Err(Error::InvalidLength);
        }
        Ok(())
    }

    pub(crate) fn read_len(&mut self) -> Result<usize, Error> {
        // FIXME:
        Ok(0)
    }

    pub(crate) fn read_str(&mut self) -> Result<&'a str, Error> {
        core::str::from_utf8(self.read_bytes()?).map_err(|_| Error::InvalidString)
    }

    pub(crate) fn read_bytes(&mut self) -> Result<&'a [u8], Error> {
        let len = self.read_u16()? as usize;

        self.check_remaining(len)?;
        let bytes = &self.buf[self.offset..self.offset + len];
        self.offset += len;
        Ok(bytes)
    }

    pub(crate) fn read_u8(&mut self) -> Result<u8, Error> {
        self.check_remaining(1)?;
        let v = self.buf[self.offset];
        self.offset += 1;
        Ok(v)
    }

    pub(crate) fn read_u16(&mut self) -> Result<u16, Error> {
        self.check_remaining(2)?;
        let v = u16::from_le_bytes(self.buf[self.offset..(self.offset + 1)].try_into().unwrap());
        self.offset += 2;
        Ok(v)
    }

    pub(crate) fn read_payload(&mut self) -> Result<&[u8], Error> {
        // FIXME:
        let len = 0;
        self.check_remaining(len)?;
        let v = &self.buf[self.offset..(self.offset + len)];
        self.offset += len;
        Ok(v)
    }

    #[cfg(feature = "mqttv5")]
    pub(crate) fn read_properties(&mut self) -> Result<Properties<'a>, Error> {
        self.check_remaining(0)?;
        Ok(Properties::Slice(&[]))
    }
}
