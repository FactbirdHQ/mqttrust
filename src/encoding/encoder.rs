use super::{error::Error, utils::Pid, FixedHeader, QoS};

#[cfg(feature = "mqttv5")]
use crate::Properties;

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
            self.write_length(v.remaining_len())?;
        } else {
            self.check_remaining(v.remaining_len())?;
        }

        Ok(())
    }

    pub(crate) fn check_remaining(&self, len: usize) -> Result<(), Error> {
        if self.buf[self.offset..].len() < len {
            Err(Error::WriteZero)
        } else {
            Ok(())
        }
    }

    pub(crate) fn write_length(&mut self, len: usize) -> Result<(), Error> {
        let write_len = match len {
            0..=127 => len + 1,
            128..=16383 => len + 2,
            16384..=2097151 => len + 3,
            2097152..=268435455 => len + 4,
            _ => return Err(Error::InvalidLength),
        };

        self.check_remaining(write_len)?;

        let mut done = false;
        let mut x = len;
        while !done {
            let mut byte = (x % 128) as u8;
            x /= 128;
            if x > 0 {
                byte |= 128;
            }
            self.write_u8(byte)?;
            done = x == 0;
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
        self.write_slice(&v.to_be_bytes())?;
        Ok(())
    }

    pub(crate) fn write_slice(&mut self, v: &[u8]) -> Result<(), Error> {
        self.write_u16(v.len() as u16)?;
        self.write_payload(v)?;

        Ok(())
    }

    pub(crate) fn write_payload(&mut self, v: &[u8]) -> Result<(), Error> {
        self.buf[self.offset..self.offset + v.len()].copy_from_slice(v);
        self.offset += v.len();

        Ok(())
    }

    #[cfg(feature = "mqttv5")]
    pub(crate) fn write_properties(&mut self, _v: &Properties<'_>) -> Result<(), Error> {
        self.write_u16(0)?;
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
