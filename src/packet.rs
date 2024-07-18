use embedded_io_async::{Error, ErrorKind, Read, Write};

use crate::{
    decoder::MqttDecoder,
    encoder::{MqttEncode, MqttEncoder},
    encoding::received_packet::ReceivedPacket,
    transport::Transport,
    PacketType, StateError,
};

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

    pub fn reset(&mut self) {
        self.read_bytes = 0;
        self.packet_len = None;
    }

    pub async fn write_packet(
        &mut self,
        transport: &mut impl Transport,
        packet: impl MqttEncode,
    ) -> Result<(), StateError> {
        // FIXME: Reuse packet buffer?
        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        packet
            .to_buffer(&mut encoder)
            .map_err(|_| StateError::Deserialization)?;

        transport
            .socket()?
            .write_all(encoder.packet_bytes())
            .await
            .map_err(|e| StateError::Io(e.kind()))?;

        transport
            .socket()?
            .flush()
            .await
            .map_err(|e| StateError::Io(e.kind()))?;
        Ok(())
    }

    pub async fn get_received_packet<'a, T: Transport>(
        &'a mut self,
        transport: &'a mut T,
    ) -> Result<ReceivedPacket<'a, T::Socket>, StateError> {
        while self.packet_len.is_none() {
            self.receive(transport.socket()?).await.map_err(|kind| {
                error!("DISCONNECTING {:?}", kind);
                transport.disconnect().ok();
                StateError::Io(kind)
            })?;
        }

        self.received_packet(transport.socket()?)
            .map_err(|_| StateError::Deserialization)
    }

    fn receive_buffer(&mut self) -> Result<&mut [u8], crate::encoding::EncodingError> {
        if self.packet_len.is_none() {
            match self.probe_fixed_header() {
                Ok(_) | Err(crate::encoding::EncodingError::InvalidLength) => {}
                Err(e) => return Err(e),
            }
        }

        let end = core::cmp::min(
            self.packet_len.unwrap_or(self.read_bytes + 1),
            self.buf.len(),
        );

        Ok(&mut self.buf[self.read_bytes..end])
    }

    fn received_packet<'a, R: Read>(
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

    fn probe_fixed_header(&mut self) -> Result<(), crate::encoding::EncodingError> {
        let mut decoder = MqttDecoder::try_new(&self.buf[..self.read_bytes])?;

        if decoder.fixed_header().typ == PacketType::Publish {
            let _topic_name = decoder.read_str()?;
            decoder.check_remaining(2)?;
        }

        self.packet_len = Some(decoder.packet_len());

        Ok(())
    }

    async fn receive<S: Read>(&mut self, socket: &mut S) -> Result<(), ErrorKind> {
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
        if received_bytes > 0 {
            self.read_bytes += received_bytes;
            Ok(())
        } else {
            Err(ErrorKind::NotConnected)
        }
    }
}
