use super::{error::Error, packets::PacketType, utils::QoS};

#[cfg(feature = "mqttv5")]
use crate::Properties;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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
    header_len: usize,
    fixed_header: FixedHeader,
}

impl<'a> MqttDecoder<'a> {
    pub fn try_new(buf: &'a [u8]) -> Result<Self, Error> {
        if buf.len() < 2 {
            return Err(Error::InvalidLength);
        }

        let hd = buf[0];
        let typ = PacketType::try_from(hd & 0b1111_0000).map_err(|_| Error::InvalidHeader)?;
        let (remaining_len, header_len) = Self::read_len(&buf[1..])?;

        let fixed_header = FixedHeader {
            typ,
            dup: hd & 0b1000 != 0,
            qos: QoS::try_from((hd & 0b0110) >> 1).map_err(|_| Error::WrongQos(hd & 0b0110))?,
            retain: hd & 0b0001 == 0b0001,
            remaining_len,
        };

        Ok(Self {
            buf,
            header_len,
            offset: header_len,
            fixed_header,
        })
    }

    pub fn fixed_header(&self) -> FixedHeader {
        self.fixed_header
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    pub fn packet_len(&self) -> usize {
        self.header_len + self.fixed_header.remaining_len
    }

    pub fn check_remaining(&self, len: usize) -> Result<(), Error> {
        if self.buf[self.offset..].len() < len {
            return Err(Error::InvalidLength);
        }
        Ok(())
    }

    fn read_len(buf: &[u8]) -> Result<(usize, usize), Error> {
        let mut integer = 0;

        for (i, byte) in buf.iter().take(4).enumerate() {
            integer += (*byte as usize & 0x7f) << (7 * i);

            if (*byte & 0b1000_0000) == 0 {
                return Ok((integer, i + 2));
            }
            if i == 3 {
                return Err(Error::MalformedPacket);
            }
        }
        Err(Error::InvalidLength)
    }

    pub(crate) fn read_varint(&mut self) -> Result<u32, Error> {
        let mut integer = 0;
        for (i, byte) in self.buf.iter().take(4).enumerate() {
            integer += (*byte as u32 & 0x7f) << (7 * i);
            if (*byte & 0b1000_0000) == 0 {
                self.offset += i + 1;
                return Ok(integer);
            }
            if i == 3 {
                return Err(Error::MalformedPacket);
            }
        }
        Err(Error::InvalidLength)
    }

    pub(crate) fn read_str(&mut self) -> Result<&'a str, Error> {
        core::str::from_utf8(self.read_slice()?).map_err(|_| Error::InvalidString)
    }

    pub(crate) fn read_slice(&mut self) -> Result<&'a [u8], Error> {
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
        let v = u16::from_be_bytes(
            self.buf[self.offset..(self.offset + 2)]
                .try_into()
                .map_err(|_| Error::MalformedPacket)?,
        );
        self.offset += 2;
        Ok(v)
    }

    pub(crate) fn read_payload(&mut self) -> Result<&'a [u8], Error> {
        let len = self.packet_len() - self.offset();
        self.check_remaining(len)?;
        let v = &self.buf[self.offset..(self.offset + len)];
        self.offset += len;
        Ok(v)
    }

    #[cfg(feature = "mqttv5")]
    pub(crate) fn read_properties(&mut self) -> Result<Properties<'a>, Error> {
        self.check_remaining(0)?;
        let len = self.read_varint()?;
        if len != 0 {
            warn!("PROPERTIES FOUND!");
        }

        // Ok(Properties::DataBlock(self.read_slice().unwrap_or_default()))
        return Ok(Properties::Slice(&[]));
    }
}
