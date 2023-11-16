use core::fmt::Debug;

use embedded_io_async::{Error, ErrorKind, Read};

use crate::encoding::{
    decoder::{read_bytes, read_header},
    packet::PacketType,
    Connack, Pid, QoS, QosPid,
};

pub(crate) struct PacketBuffer<const N: usize> {
    // MqttStack holds an RX buffer just big enough to hold a single packet
    // header + `MAX_TOPIC_LEN`, in order for it to handle `ACK`, `PING`, and
    // route incoming `PUBLISH` messages to a subscriber.
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> PacketBuffer<N> {
    pub fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    fn buf(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn rotate(&mut self, amt: usize) {
        let amt = core::cmp::min(self.len, amt);
        self.buf.rotate_left(amt);
        self.len -= amt;
    }

    pub(crate) fn has_header(&self) -> bool {
        let mut offset = 0;
        match read_header(self.buf(), &mut offset) {
            Ok(Some(_)) => true,
            _ => false,
        }
    }

    pub(crate) async fn receive<S: Read>(&mut self, socket: &mut S) -> Result<(), ErrorKind> {
        match socket.read(&mut self.buf[self.len..]).await {
            Ok(0) => Err(ErrorKind::NotConnected),
            Ok(n) => {
                self.len += n;
                Ok(())
            }
            Err(e) => Err(e.kind()),
        }
    }
}

pub(crate) enum ReceivedPacket<'a, R: Read> {
    Pingresp,
    Connack(Connack),
    Publish(QosPid, LazyPublish<'a, R, 128>),
    Puback(Pid),
    #[cfg(feature = "qos2")]
    Pubrec(Pid),
    #[cfg(feature = "qos2")]
    Pubrel(Pid),
    #[cfg(feature = "qos2")]
    Pubcomp(Pid),
    Suback(Pid), // FIXME: missing result codes
    Unsuback(Pid),
}

impl<'a, R: Read> Debug for ReceivedPacket<'a, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pingresp => write!(f, "Pingresp"),
            Self::Connack(arg0) => f.debug_tuple("Connack").field(arg0).finish(),
            Self::Publish(arg0, _) => f.debug_tuple("Publish").field(arg0).finish(),
            Self::Puback(arg0) => f.debug_tuple("Puback").field(arg0).finish(),
            Self::Pubrec(arg0) => f.debug_tuple("Pubrec").field(arg0).finish(),
            Self::Pubrel(arg0) => f.debug_tuple("Pubrel").field(arg0).finish(),
            Self::Pubcomp(arg0) => f.debug_tuple("Pubcomp").field(arg0).finish(),
            Self::Suback(arg0) => f.debug_tuple("Suback").field(arg0).finish(),
            Self::Unsuback(arg0) => f.debug_tuple("Unsuback").field(arg0).finish(),
        }
    }
}

impl<'a, R: Read> ReceivedPacket<'a, R> {
    pub(crate) fn try_new<const N: usize>(
        buf: &'a mut PacketBuffer<N>,
        reader: &'a mut R,
    ) -> Option<Self> {
        let mut offset = 0;

        let (header, remaining_len) = match read_header(buf.buf(), &mut offset) {
            Ok(Some((header, remaining_len))) => (header, remaining_len),
            Ok(None) => {
                // Need more data
                return None;
            }
            Err(_) => {
                // FIXME: Is this correct?
                error!(
                    "Unable to read incoming packet header! Discarding one byte. BUF: {:?}",
                    buf.buf()
                );
                buf.rotate(1);
                return None;
            }
        };

        debug!("Read header {:?}", header.typ);

        let packet_len = offset + remaining_len;

        let packet = Some(match header.typ {
            PacketType::Pingresp => Self::Pingresp,
            PacketType::Connack => {
                Self::Connack(Connack::from_buffer(buf.buf(), &mut offset).ok()?)
            }
            PacketType::Publish => {
                let _topic_name = read_bytes(buf.buf(), &mut offset).ok()?;

                let qos_pid = match header.qos {
                    QoS::AtMostOnce => QosPid::AtMostOnce,
                    QoS::AtLeastOnce => {
                        QosPid::AtLeastOnce(Pid::from_buffer(buf.buf(), &mut offset).ok()?)
                    }
                    #[cfg(feature = "qos2")]
                    QoS::ExactlyOnce => {
                        QosPid::ExactlyOnce(Pid::from_buffer(buf.buf(), &mut offset).ok()?)
                    }
                };

                let buf_len = core::cmp::min(packet_len, buf.buf().len());

                Self::Publish(
                    qos_pid,
                    LazyPublish::new(&buf.buf()[..buf_len], packet_len, reader),
                )
            }
            PacketType::Puback => Self::Puback(Pid::from_buffer(buf.buf(), &mut offset).ok()?),
            #[cfg(feature = "qos2")]
            PacketType::Pubrec => Self::Pubrec(Pid::from_buffer(buf.buf(), &mut offset).ok()?),
            #[cfg(feature = "qos2")]
            PacketType::Pubrel => Self::Pubrel(Pid::from_buffer(buf.buf(), &mut offset).ok()?),
            #[cfg(feature = "qos2")]
            PacketType::Pubcomp => Self::Pubcomp(Pid::from_buffer(buf.buf(), &mut offset).ok()?),
            PacketType::Suback => Self::Suback(Pid::from_buffer(buf.buf(), &mut offset).ok()?),
            PacketType::Unsuback => Self::Unsuback(Pid::from_buffer(buf.buf(), &mut offset).ok()?),
            _ => return None,
        });

        buf.rotate(packet_len);

        packet
    }
}

pub(crate) struct LazyPublish<'a, S: Read, const N: usize> {
    buf: heapless::Vec<u8, N>,
    packet_len: usize,
    reader: &'a mut S,
}

impl<'a, S: Read, const N: usize> LazyPublish<'a, S, N> {
    pub fn new(buf: &[u8], packet_len: usize, reader: &'a mut S) -> Self {
        Self {
            buf: heapless::Vec::from_slice(buf).unwrap(),
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
