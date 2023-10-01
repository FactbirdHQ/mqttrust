use crate::de;
use embedded_io_async::Read;

use super::packet_header::PacketHeader;
use crate::error::ProtocolError;

pub(crate) struct PacketReader {
    buf: [u8; 32],
    read_bytes: usize,
    packet_length: Option<usize>,
}

impl PacketReader {
    pub fn new() -> PacketReader {
        PacketReader {
            buf: [0; 32],
            read_bytes: 0,
            packet_length: None,
        }
    }

    pub async fn next_packet_header<R: Read>(
        &mut self,
        mut reader: R,
    ) -> Result<PacketHeader, ProtocolError> {
        if let Some(packet_length) = self.packet_length.take() {
            if packet_length < self.read_bytes {
                return Err(ProtocolError::MalformedPacket);
            }
            self.read_bytes -= packet_length;
        }

        while !self.buf.is_empty() {
            match reader.read(&mut self.buf[self.read_bytes..]).await {
                Ok(0) => return Err(ProtocolError::Deserialization(de::Error::InsufficientData)),
                Ok(n) => {
                    self.read_bytes += n;

                    if self.probe_fixed_header()? {
                        break;
                    }
                }
                Err(_e) => return Err(ProtocolError::BadIdentifier),
            }
        }

        PacketHeader::from_buffer(&self.buf[..self.read_bytes])
    }

    /// Check for the presence of a full MQTT fixed header
    ///
    /// Returns `Some((header_len, packet_len))` if a fixed header is found
    fn probe_fixed_header(&mut self) -> Result<bool, ProtocolError> {
        if self.read_bytes <= 1 {
            return Ok(false);
        }

        self.packet_length = None;

        let mut packet_length = 0;
        for (index, value) in self.buf[1..self.read_bytes].iter().take(4).enumerate() {
            packet_length += ((value & 0x7F) as usize) << (index * 7);
            if (value & 0x80) == 0 {
                let length_size_bytes = 1 + index;

                // MQTT headers encode the packet type in the first byte followed by the packet
                // length as a varint
                let header_size_bytes = 1 + length_size_bytes;
                self.packet_length = Some(header_size_bytes + packet_length);
                break;
            }
        }

        // We should have found the packet length by now.
        if self.read_bytes >= 5 && self.packet_length.is_none() {
            return Err(ProtocolError::MalformedPacket);
        }

        Ok(true)
    }

    pub fn reset(&mut self) {
        self.read_bytes = 0;
        self.packet_length = None;
    }
}
