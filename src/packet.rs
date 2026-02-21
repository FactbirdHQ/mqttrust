use embedded_io_async::{Error, ErrorKind, Read, Write};

use crate::{
    crate_config::MAX_TOPIC_LEN,
    decoder::MqttDecoder,
    encoder::{MqttEncode, MqttEncoder, MAX_MQTT_HEADER_LEN},
    encoding::received_packet::ReceivedPacket,
    PacketType, StateError,
};

/// Read buffer sized to hold a single MQTT packet header + topic.
///
/// Layout: fixed header (5) + topic length prefix (2) + topic (MAX_TOPIC_LEN) + PID (2)
const READ_BUF_LEN: usize = MAX_MQTT_HEADER_LEN + 2 + MAX_TOPIC_LEN + 2;

pub(crate) struct PacketReader {
    buf: [u8; READ_BUF_LEN],
    read_bytes: usize,
    packet_len: Option<usize>,
}

impl PacketReader {
    pub fn new() -> Self {
        Self {
            buf: [0; READ_BUF_LEN],
            read_bytes: 0,
            packet_len: None,
        }
    }

    /// Reset the reader state, discarding any partially or fully received packet.
    ///
    /// This is `pub(crate)` for error-recovery use in `MqttStack::reset()`.
    pub(crate) fn reset(&mut self) {
        self.read_bytes = 0;
        self.packet_len = None;
    }

    pub async fn recv<'a, R: Read>(
        &'a mut self,
        reader: &'a mut R,
    ) -> Result<ReceivedPacket<'a, R>, StateError> {
        // Auto-reset: if a previous packet was fully received (packet_len is Some),
        // clear the buffer so we can start reading the next packet. Partial reads
        // (packet_len = None) are preserved across select3 iterations.
        if self.packet_len.is_some() {
            self.reset();
        }

        while self.packet_len.is_none() {
            self.read_chunk(reader).await.map_err(StateError::Io)?;
        }

        self.decode(reader).map_err(|_| StateError::Deserialization)
    }

    fn next_read_buf(&mut self) -> Result<&mut [u8], crate::encoding::EncodingError> {
        if self.packet_len.is_none() {
            match self.try_parse_header() {
                Ok(_) | Err(crate::encoding::EncodingError::InsufficientBytes) => {}
                Err(e) => return Err(e),
            }
        }

        let end = core::cmp::min(
            self.packet_len.unwrap_or(self.read_bytes + 1),
            self.buf.len(),
        );

        Ok(&mut self.buf[self.read_bytes..end])
    }

    fn decode<'a, R: Read>(
        &'a mut self,
        reader: &'a mut R,
    ) -> Result<ReceivedPacket<'a, R>, crate::encoding::EncodingError> {
        let packet_length = *self
            .packet_len
            .as_ref()
            .ok_or(crate::encoding::EncodingError::MalformedPacket)?;

        let buf_len = core::cmp::min(packet_length, self.read_bytes);

        ReceivedPacket::from_buffer(&self.buf[..buf_len], reader)
    }

    fn try_parse_header(&mut self) -> Result<(), crate::encoding::EncodingError> {
        let mut decoder = MqttDecoder::try_new(&self.buf[..self.read_bytes])?;

        if decoder.fixed_header().typ == PacketType::Publish {
            let _topic_name = decoder.read_str()?;
            decoder.check_remaining(2)?;
        }

        self.packet_len = Some(decoder.packet_len());

        Ok(())
    }

    async fn read_chunk<S: Read>(&mut self, socket: &mut S) -> Result<(), ErrorKind> {
        let buffer = match self.next_read_buf() {
            Ok(buffer) => buffer,
            Err(e) => {
                warn!("RESET ERROR {:?}", e);
                self.read_bytes = 0;
                self.packet_len = None;
                return Err(ErrorKind::InvalidData);
            }
        };

        if buffer.is_empty() {
            return Ok(());
        }

        let received_bytes = socket.read(buffer).await.map_err(|e| e.kind())?;
        if received_bytes > 0 {
            self.read_bytes += received_bytes;
            Ok(())
        } else {
            Err(ErrorKind::NotConnected)
        }
    }
}

/// Encode `packet` into `buf`, returning the encoded byte slice.
///
/// Non-generic over the writer type — monomorphized only per packet type.
pub(crate) fn encode_packet(buf: &mut [u8], packet: impl MqttEncode) -> Result<&[u8], StateError> {
    let mut encoder = MqttEncoder::new(buf);
    packet
        .to_buffer(&mut encoder)
        .map_err(|_| StateError::Deserialization)?;
    Ok(encoder.into_packet_bytes())
}

/// Write pre-encoded bytes to `writer` and flush.
///
/// Generic only over the writer type — single monomorphization per socket type,
/// independent of packet type.
pub(crate) async fn write_bytes<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<(), StateError> {
    writer
        .write_all(bytes)
        .await
        .map_err(|e| StateError::Io(e.kind()))?;
    writer.flush().await.map_err(|e| StateError::Io(e.kind()))?;
    Ok(())
}
