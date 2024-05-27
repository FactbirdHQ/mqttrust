use core::fmt::Debug;

use crate::{
    encoder::{TxHeader, MAX_MQTT_HEADER_LEN, TX_HEADER_LEN},
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        properties::Properties,
        utils::{Pid, QoS},
        FixedHeader,
    },
    varint_len,
};
use embedded_io_async::Read;

use super::PacketType;

pub trait ToPayload {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error>;
    fn max_size(&self) -> usize;
}

impl<'a> ToPayload for &'a [u8] {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        if buffer.len() < self.len() {
            return Err(Error::BufferSize);
        }
        buffer[..self.len()].copy_from_slice(self);
        Ok(self.len())
    }

    fn max_size(&self) -> usize {
        self.len()
    }
}

impl<const N: usize> ToPayload for [u8; N] {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        (&self[..]).serialize(buffer)
    }

    fn max_size(&self) -> usize {
        self.len()
    }
}

impl<const N: usize> ToPayload for &[u8; N] {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        (&self[..]).serialize(buffer)
    }

    fn max_size(&self) -> usize {
        self.len()
    }
}

pub struct DeferredPayload<F: Fn(&mut [u8]) -> Result<usize, Error>> {
    func: F,
    max_len: usize,
}

impl<F: Fn(&mut [u8]) -> Result<usize, Error>> DeferredPayload<F> {
    pub fn new(func: F, max_len: usize) -> Self {
        Self { func, max_len }
    }
}

impl<F: Fn(&mut [u8]) -> Result<usize, Error>> ToPayload for DeferredPayload<F> {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        if buffer.len() < self.max_len {
            return Err(Error::BufferSize);
        }
        (self.func)(buffer)
    }

    fn max_size(&self) -> usize {
        self.max_len
    }
}

/// Publish packet ([MQTT 3.3]).
///
/// [MQTT 3.3]: https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc3901100
#[derive(Debug, Clone, PartialEq)]
pub struct Publish<'a, P: ToPayload> {
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
    pub pid: Option<Pid>,
    pub topic_name: &'a str,
    pub payload: P,
    #[cfg(feature = "mqttv5")]
    pub properties: Properties<'a>,
}

impl<'a, P: ToPayload> FixedHeader for Publish<'a, P> {
    const PACKET_TYPE: PacketType = PacketType::Publish;

    fn flags(&self) -> u8 {
        let mut flags = u8::from(self.qos) << 1;
        if self.dup {
            flags |= 0b1000;
        };
        if self.retain {
            flags |= 0b0001;
        };

        flags
    }
}

impl<'a, P: ToPayload> MqttEncode for Publish<'a, P> {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<TxHeader, Error> {
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
        encoder.write_payload(&self.payload)?;

        encoder.finalize_fixed_header(self)?;

        Ok(encoder.write_tx_header(Self::PACKET_TYPE, self.get_qos(), self.pid)?)
    }

    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }

    fn get_qos(&self) -> Option<QoS> {
        Some(self.qos)
    }

    fn max_packet_size(&self) -> usize {
        let mut length = 2 + self.topic_name.len()
            + match self.qos {
                QoS::AtMostOnce => 0,
                _ => 2,
            } // pid
            + MAX_MQTT_HEADER_LEN + TX_HEADER_LEN;

        #[cfg(feature = "mqttv5")]
        {
            length += varint_len(self.properties.size());
        }

        length += self.payload.max_size();

        length
    }
}

pub(crate) struct PartialPublish<'a, S: Read> {
    buf: &'a [u8],
    packet_len: usize,
    reader: &'a mut S,
}

impl<'a, S: Read> Debug for PartialPublish<'a, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PartialPublish")
            .field("buf", &self.buf)
            .field("packet_len", &self.packet_len)
            .finish()
    }
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
        trace!("Starting multipart copy of publish packet...");
        trace!("Part 1: {} bytes", self.buf.len());
        buf[..self.buf.len()].copy_from_slice(&self.buf);

        if self.buf.len() < self.len() {
            trace!("Part 2: {} bytes", self.len() - self.buf.len());

            self.reader
                .read_exact(&mut buf[self.buf.len()..self.len()])
                .await?;
        }
        trace!("Finished multipart copy of {} bytes!", self.len());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_publish_bytes_v5() {
        let expected_bytes = &[
            50, 57, 0, 21, 109, 121, 47, 112, 101, 114, 115, 111, 110, 97, 108, 105, 122, 101, 100,
            47, 116, 111, 112, 105, 99, 0, 1, 0, 84, 104, 105, 115, 32, 105, 115, 32, 109, 121, 32,
            97, 119, 101, 115, 111, 109, 101, 32, 98, 121, 116, 101, 32, 112, 97, 121, 108, 111,
            97, 100,
        ];

        let publish = Publish {
            dup: false,
            qos: QoS::AtLeastOnce,
            retain: false,
            pid: Some(Pid::new()),
            topic_name: "my/personalized/topic",
            payload: b"This is my awesome byte payload",
            properties: Properties::Slice(&[]),
        };

        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        publish.to_buffer(&mut encoder).unwrap();

        assert_eq!(publish.max_packet_size(), 67);
        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_publish_closure_v5() {
        let expected_bytes = &[
            50, 57, 0, 21, 109, 121, 47, 112, 101, 114, 115, 111, 110, 97, 108, 105, 122, 101, 100,
            47, 116, 111, 112, 105, 99, 0, 1, 0, 84, 104, 105, 115, 32, 105, 115, 32, 109, 121, 32,
            97, 119, 101, 115, 111, 109, 101, 32, 98, 121, 116, 101, 32, 112, 97, 121, 108, 111,
            97, 100,
        ];

        let payload_bytes = b"This is my awesome byte payload";

        let payload_closure = |buf: &mut [u8]| {
            buf[..payload_bytes.len()].copy_from_slice(payload_bytes);
            Ok(payload_bytes.len())
        };

        let publish = Publish {
            dup: false,
            qos: QoS::AtLeastOnce,
            retain: false,
            pid: Some(Pid::new()),
            topic_name: "my/personalized/topic",
            payload: DeferredPayload::new(payload_closure, payload_bytes.len()),
            properties: Properties::Slice(&[]),
        };

        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        publish.to_buffer(&mut encoder).unwrap();

        assert_eq!(publish.max_packet_size(), 67);
        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }
}
