use super::{error::Error, packets::PacketType, utils::QoS};

#[cfg(feature = "mqttv5")]
use crate::Properties;
/// Represents the fixed header of an MQTT packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct FixedHeader {
    /// The type of the MQTT packet.
    pub typ: PacketType,
    /// Whether the packet is a duplicate.
    pub dup: bool,
    /// The quality of service (QoS) of the packet.
    pub qos: QoS,
    /// Whether the packet is retained.
    pub retain: bool,
    /// The remaining length of the packet.
    pub remaining_len: usize,
}

/// A struct that implements the MQTT decoder.
pub struct MqttDecoder<'a> {
    /// The buffer containing the MQTT packet.
    buf: &'a [u8],
    /// The current offset in the buffer.
    offset: usize,
    /// The length of the fixed header.
    header_len: usize,
    /// The fixed header of the packet.
    fixed_header: FixedHeader,
}

impl<'a> MqttDecoder<'a> {
    const TYP_MASK: u8 = 0b1111_0000;
    const DUP_MASK: u8 = 0b0000_1000;
    const QOS_MASK: u8 = 0b0000_0110;
    const RETAIN_MASK: u8 = 0b0000_0001;

    const CONT_BITMASK: u8 = 0b1000_0000;

    /// Tries to create a new `MqttDecoder` from the given buffer.
    pub fn try_new(buf: &'a [u8]) -> Result<Self, Error> {
        if buf.len() < 2 {
            return Err(Error::InsufficientBytes);
        }

        let hd = buf[0];
        let typ = PacketType::try_from(hd & Self::TYP_MASK).map_err(|_| Error::InvalidHeader)?;
        let (remaining_len, header_len) = Self::read_len(&buf[1..])?;

        let fixed_header = FixedHeader {
            typ,
            dup: (hd & Self::DUP_MASK) == Self::DUP_MASK,
            qos: QoS::try_from((hd & Self::QOS_MASK) >> 1)
                .map_err(|_| Error::WrongQos(hd & Self::QOS_MASK))?,
            retain: (hd & Self::RETAIN_MASK) == Self::RETAIN_MASK,
            remaining_len,
        };

        Ok(Self {
            buf,
            header_len,
            offset: header_len,
            fixed_header,
        })
    }

    /// Returns the fixed header of the packet.
    pub fn fixed_header(&self) -> FixedHeader {
        self.fixed_header
    }

    /// Returns the current offset in the buffer.
    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the total length of the packet.
    pub fn packet_len(&self) -> usize {
        self.header_len + self.fixed_header.remaining_len
    }

    /// Checks if there is enough data remaining in the buffer to read the specified number of bytes.
    pub fn check_remaining(&self, len: usize) -> Result<(), Error> {
        if self.buf[self.offset..].len() < len {
            return Err(Error::InsufficientBytes);
        }
        Ok(())
    }

    /// Reads the remaining length from the buffer.
    fn read_len(buf: &[u8]) -> Result<(usize, usize), Error> {
        let mut integer = 0;

        for (i, byte) in buf.iter().take(4).enumerate() {
            integer += ((*byte & !Self::CONT_BITMASK) as usize) << (7 * i);

            if (*byte & Self::CONT_BITMASK) == 0 {
                return Ok((integer, i + 2));
            }
            if i == 3 {
                return Err(Error::MalformedPacket);
            }
        }
        Err(Error::InsufficientBytes)
    }

    /// Reads a variable length integer from the buffer.
    pub(crate) fn read_varint(&mut self) -> Result<u32, Error> {
        let mut integer = 0;
        for (i, byte) in self.buf[self.offset..].iter().take(4).enumerate() {
            integer += ((*byte & !Self::CONT_BITMASK) as u32) << (7 * i);

            if (*byte & Self::CONT_BITMASK) == 0 {
                self.offset += i + 1;
                return Ok(integer);
            }
            if i == 3 {
                return Err(Error::MalformedPacket);
            }
        }
        Err(Error::InsufficientBytes)
    }

    /// Reads a string from the buffer.
    pub(crate) fn read_str(&mut self) -> Result<&'a str, Error> {
        core::str::from_utf8(self.read_slice()?).map_err(|_| Error::InvalidString)
    }

    /// Reads a byte slice from the buffer.
    pub(crate) fn read_slice(&mut self) -> Result<&'a [u8], Error> {
        let len = self.read_u16()? as usize;

        self.check_remaining(len)?;
        let bytes = &self.buf[self.offset..self.offset + len];
        self.offset += len;
        Ok(bytes)
    }

    /// Reads a single byte from the buffer.
    pub(crate) fn read_u8(&mut self) -> Result<u8, Error> {
        self.check_remaining(1)?;
        let v = self.buf[self.offset];
        self.offset += 1;
        Ok(v)
    }

    /// Reads a 16-bit unsigned integer from the buffer.
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

    /// Reads the payload of the packet from the buffer.
    pub(crate) fn read_payload(&mut self) -> Result<&'a [u8], Error> {
        let len = self.packet_len() - self.offset();
        self.check_remaining(len)?;
        let v = &self.buf[self.offset..(self.offset + len)];
        self.offset += len;
        Ok(v)
    }

    /// Reads MQTTv5 properties from the buffer.
    #[cfg(feature = "mqttv5")]
    pub(crate) fn read_properties(&mut self) -> Result<Properties<'a>, Error> {
        let len = self.read_varint()? as usize;
        self.check_remaining(len)?;
        let v = &self.buf[self.offset..(self.offset + len)];
        self.offset += len;

        let properties = Properties::DataBlock(v);
        if properties.size() > 0 {
            info!("GOT PROPS");
            for prop in properties.iter() {
                info!("{:?}", prop);
            }
        }

        Ok(properties)
    }
}

/// A trait for decoding MQTT packets from a buffer.
pub trait MqttDecode<'a>: Sized {
    /// Decodes the packet from a given decoder.
    fn from_decoder(decoder: &mut MqttDecoder<'a>) -> Result<Self, Error>;
}
