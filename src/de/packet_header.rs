use crate::{
    error::ProtocolError,
    message_types::{self, MessageType},
    varint::Varint,
    QoS,
};
use bit_field::BitField;
use embedded_io_async::Read;
use serde::Deserialize;

#[derive(Debug)]
pub(crate) struct PacketHeader<'a> {
    buf: &'a [u8],
}

impl<'a> PacketHeader<'a> {
    pub fn from_buffer(buf: &'a [u8]) -> Result<Self, ProtocolError> {
        Ok(Self { buf })
    }

    pub fn len(&self) -> usize {
        self.header_len() + self.payload_len()
    }

    fn header_len(&self) -> usize {
        11
    }

    fn payload_len(&self) -> usize {
        0
    }

    pub fn tester(&self) -> u8 {
        self.buf[0].get_bits(4..8)
    }

    pub fn message_type(&self) -> message_types::MessageType {
        MessageType::try_from(self.buf[0].get_bits(4..8)).unwrap()
    }

    pub fn qos(&self) -> Option<QoS> {
        match self.message_type() {
            MessageType::Publish | MessageType::SubAck => Some(QoS::AtLeastOnce),
            _ => None,
        }
    }

    pub fn pid(&self) -> Option<u16> {
        match self.message_type() {
            MessageType::Publish | MessageType::SubAck => Some(0),
            _ => None,
        }
    }

    pub fn topic(&self) -> Option<&str> {
        match self.message_type() {
            MessageType::Publish => core::str::from_utf8(&self.buf[10..15]).ok(),
            _ => None,
        }
    }

    pub fn publish_info(&self) -> Option<(QoS, u16)> {
        if !matches!(self.message_type(), MessageType::Publish) {
            return None;
        }

        Some((QoS::AtLeastOnce, 0))
    }

    pub async fn copy_packet<R: Read>(
        &self,
        buf: &mut [u8],
        mut reader: R,
    ) -> Result<usize, ProtocolError> {
        if buf.len() < self.len() {
            return Err(ProtocolError::BufferSize);
        }

        // If the full MQTT packet is larger than what is in `self.buf`, we need to read more bytes
        if self.len() > self.buf.len() {
            buf[..self.buf.len()].copy_from_slice(self.buf);
            reader
                .read_exact(&mut buf[self.buf.len()..self.len()])
                .await
                .unwrap();
        } else {
            buf[..self.len()].copy_from_slice(&self.buf[..self.len()]);
        }
        Ok(self.len())
    }
}

// struct PacketHeaderVisitor;

// impl<'de> serde::de::Visitor<'de> for PacketHeaderVisitor {
//     type Value = PacketHeader<'de>;

//     fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
//         write!(formatter, "MQTT Control Packet")
//     }

//     fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
//         use serde::de::Error;

//         // Note(unwraps): These unwraps should never fail - the next_element() function should be
//         // always providing us some new element or an error that we return based on our
//         // deserialization implementation.
//         let fixed_header: u8 = seq.next_element()?.unwrap();
//         let _length: Varint = seq.next_element()?.unwrap();
//         let packet_type = MessageType::try_from(fixed_header.get_bits(4..=7))
//             .map_err(|_| A::Error::custom("Invalid MQTT control packet type"))?;

//         Ok(PacketHeader { buf: self. })
//     }
// }

// impl<'de> Deserialize<'de> for PacketHeader<'de> {
//     fn deserialize<D: serde::de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
//         // Deserialize the (fixed_header, length, control_packet | (topic, packet_id, properties)),
//         // which corresponds to a maximum of 5 elements.
//         deserializer.deserialize_tuple(5, PacketHeaderVisitor)
//     }
// }
