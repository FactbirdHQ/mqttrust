use embedded_io_async::{Error, ErrorKind, Read};

use crate::encoding::received_packet::ReceivedPacket;

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

    pub fn receive_buffer(&mut self) -> Result<&mut [u8], ErrorKind> {
        if self.packet_len.is_none() {
            self.probe_fixed_header()?;
        }

        let end = if let Some(packet_len) = &self.packet_len {
            *packet_len
        } else {
            self.read_bytes + 1
        };

        Ok(&mut self.buf[self.read_bytes..end])
    }

    pub fn commit(&mut self, count: usize) {
        self.read_bytes += count;
    }

    fn probe_fixed_header(&mut self) -> Result<(), ErrorKind> {
        if self.read_bytes <= 1 {
            return Ok(());
        }

        self.packet_len = None;

        let mut packet_len = 0;
        for (index, value) in self.buf[1..self.read_bytes].iter().take(4).enumerate() {
            packet_len += ((value & 0x7F) as usize) << (index * 7);
            if (value & 0x80) == 0 {
                let length_size_bytes = 1 + index;

                // MQTT headers encode the packet type in the first byte followed by the packet
                // length as a varint
                let header_size_bytes = 1 + length_size_bytes;
                self.packet_len = Some(header_size_bytes + packet_len);
                break;
            }
        }

        // We should have found the packet length by now.
        if self.read_bytes >= 5 && self.packet_len.is_none() {
            return Err(ErrorKind::InvalidData);
        }

        Ok(())
    }

    pub fn packet_available(&self) -> bool {
        match self.packet_len {
            Some(len) => self.read_bytes >= len,
            None => false,
        }
    }

    pub fn reset(&mut self) {
        self.read_bytes = 0;
        self.packet_len = None;
    }

    pub fn received_packet<'a, R: Read>(
        &'a mut self,
        reader: &'a mut R,
    ) -> Result<ReceivedPacket<'a, R, 128>, crate::encoding::Error> {
        let packet_len = *self
            .packet_len
            .as_ref()
            .ok_or(crate::encoding::Error::InvalidLength)?;

        // Reset the buffer now. Once the user drops the `ReceivedPacket`, this reader will then be
        // immediately ready to begin receiving a new packet.
        self.reset();

        ReceivedPacket::from_buffer(&self.buf[..packet_len], reader)
    }

    pub(crate) async fn receive<S: Read>(&mut self, socket: &mut S) -> Result<(), ErrorKind> {
        let buffer = self.receive_buffer()?;

        match socket.read(buffer).await {
            Ok(0) => Err(ErrorKind::NotConnected),
            Ok(n) => {
                self.commit(n);
                Ok(())
            }
            Err(e) => Err(e.kind()),
        }
    }
}
