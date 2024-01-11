use embedded_io_async::{Error, ErrorKind, Read};

use crate::{decoder::MqttDecoder, encoding::received_packet::ReceivedPacket, PacketType};

pub(crate) struct PacketBuffer<const N: usize> {
    // MqttStack holds an RX buffer just big enough to hold a single packet
    // header + `MAX_TOPIC_LEN`, in order for it to handle `ACK`, `PING`, and
    // route incoming `PUBLISH` messages to a subscriber.
    buf: [u8; N],
    read_bytes: usize,
    packet_len: Option<usize>,
}

impl<const N: usize> PacketBuffer<N> {
    pub fn new() -> Self {
        Self {
            buf: [0; N],
            read_bytes: 0,
            packet_len: None,
        }
    }

    pub fn commit(&mut self, count: usize) {
        self.read_bytes += count;
    }

    pub fn receive_buffer(&mut self) -> Result<&mut [u8], crate::encoding::EncodingError> {
        if self.packet_len.is_none() {
            match self.probe_fixed_header() {
                Ok(_) | Err(crate::encoding::EncodingError::InvalidLength) => {}
                Err(e) => return Err(e),
            }
        }

        let end = if let Some(packet_length) = self.packet_len {
            packet_length
        } else {
            self.read_bytes + 1
        };

        let end = core::cmp::min(end, self.buf.len());

        Ok(&mut self.buf[self.read_bytes..end])
    }

    pub fn received_packet<'a, R: Read>(
        &'a mut self,
        reader: &'a mut R,
    ) -> Result<ReceivedPacket<'a, R>, crate::encoding::EncodingError> {
        let packet_length = *self
            .packet_len
            .as_ref()
            .ok_or(crate::encoding::EncodingError::MalformedPacket)?;

        let buf_len = core::cmp::min(packet_length, self.read_bytes);

        let packet = ReceivedPacket::from_buffer(&self.buf[..buf_len], reader)?;

        // Reset the buffer now. Once the user drops the `ReceivedPacket`, this reader will then be
        // immediately ready to begin receiving a new packet.

        self.read_bytes = 0;
        self.packet_len = None;

        Ok(packet)
    }

    pub fn probe_fixed_header(&mut self) -> Result<(), crate::encoding::EncodingError> {
        let mut decoder = MqttDecoder::try_new(&self.buf[..self.read_bytes])?;

        if decoder.fixed_header().typ == PacketType::Publish {
            let _topic_name = decoder.read_str()?;
            decoder.check_remaining(2)?;
        }

        self.packet_len = Some(decoder.packet_len());

        Ok(())
    }

    pub fn packet_available(&mut self) -> bool {
        self.packet_len.is_some()
    }

    pub(crate) async fn receive<S: Read>(&mut self, socket: &mut S) -> Result<(), ErrorKind> {
        let buffer = match self.receive_buffer() {
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
        self.commit(received_bytes);

        if received_bytes > 0 {
            Ok(())
        } else {
            Err(ErrorKind::NotConnected)
        }
    }
}
