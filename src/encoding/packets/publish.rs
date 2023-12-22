use crate::encoding::{
    decoder::MqttDecoder,
    encoder::{MqttEncode, MqttEncoder},
    error::Error,
    properties::Properties,
    utils::{Pid, QoS},
    FixedHeader,
};
use embedded_io_async::Read;

use super::PacketType;

/// Publish packet ([MQTT 3.3]).
///
/// [MQTT 3.3]: https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc3901100
#[derive(Debug, Clone, PartialEq)]
pub struct Publish<'a> {
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
    pub pid: Option<Pid>,
    pub topic_name: &'a str,
    pub payload: &'a [u8],
    pub properties: Properties<'a>,
}

impl<'a> Publish<'a> {
    pub fn from_buffer(decoder: &'a mut MqttDecoder) -> Result<Self, Error> {
        let header = decoder.read_fixed_header()?;

        if !matches!(header.typ, Self::PACKET_TYPE) {
            return Err(Error::InvalidHeader);
        }

        // Topic
        let topic_name = decoder.read_str()?;

        let pid = match header.qos {
            QoS::AtMostOnce => None,
            QoS::AtLeastOnce => {
                Some(Pid::try_from(decoder.read_u16()?).map_err(|_| Error::PidMissing)?)
            }
            #[cfg(feature = "qos2")]
            QoS::ExactlyOnce => {
                Some(Pid::try_from(decoder.read_u16()?).map_err(|_| Error::PidMissing)?)
            }
        };

        Ok(Self {
            dup: header.dup,
            qos: header.qos,
            retain: header.retain,
            pid,
            topic_name,
            #[cfg(feature = "mqttv5")]
            properties: decoder.read_properties()?,
            payload: decoder.read_payload()?,
        })
    }
}

impl<'a> FixedHeader for Publish<'a> {
    const PACKET_TYPE: PacketType = PacketType::Publish;

    fn flags(&self) -> u8 {
        let mut flags: u8 = self.qos.into();
        if self.dup {
            flags |= 0b1000;
        };
        if self.retain {
            flags |= 0b0001;
        };

        flags
    }

    fn remaining_len(&self) -> usize {
        2 + self.topic_name.len()
            + match self.qos {
                QoS::AtMostOnce => 0,
                _ => 2,
            }
            // FIXME:
            // + self.properties.len()
            + self.payload.len()
    }
}

impl<'a> MqttEncode for Publish<'a> {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_fixed_header(self)?;

        // Topic
        encoder.write_str(self.topic_name)?;

        match self.qos {
            QoS::AtMostOnce => (),
            QoS::AtLeastOnce => {
                encoder.write_u16(self.pid.ok_or(Error::PidMissing)?.get())?;
            }
            #[cfg(feature = "qos2")]
            QoS::ExactlyOnce => {
                encoder.write_u16(self.pid.ok_or(Error::PidMissing)?.get())?;
            }
        }

        // Properties
        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        // Payload
        encoder.write_payload(self.payload)?;

        Ok(())
    }

    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }

    fn get_qos(&self) -> Option<QoS> {
        Some(self.qos)
    }
}

pub(crate) struct PartialPublish<'a, S: Read> {
    buf: &'a [u8],
    packet_len: usize,
    reader: &'a mut S,
}

impl<'a, S: Read> PartialPublish<'a, S> {
    pub fn new(buf: &'a [u8], packet_len: usize, reader: &'a mut S) -> Self {
        Self {
            buf,
            packet_len,
            reader,
        }
    }

    pub fn len(&self) -> usize {
        self.packet_len
    }

    pub async fn copy_all(
        &mut self,
        buf: &mut [u8],
    ) -> Result<(), embedded_io_async::ReadExactError<S::Error>> {
        buf[..self.buf.len()].copy_from_slice(&self.buf);

        if self.buf.len() < self.packet_len {
            self.reader
                .read_exact(&mut buf[self.buf.len()..self.packet_len])
                .await?;
        }

        Ok(())
    }
}
