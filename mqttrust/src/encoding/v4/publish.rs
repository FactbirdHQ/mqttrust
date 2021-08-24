use super::{decoder::*, encoder::*, *};

/// Publish packet ([MQTT 3.3]).
///
/// [MQTT 3.3]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718037
#[derive(Debug, Clone, PartialEq)]
pub struct Publish<'a> {
    pub dup: bool,
    pub qos: QoS,
    pub pid: Option<Pid>,
    pub retain: bool,
    pub topic_name: &'a str,
    pub payload: &'a [u8],
}

impl<'a> Publish<'a> {
    pub(crate) fn from_buffer(
        header: &Header,
        remaining_len: usize,
        buf: &'a [u8],
        offset: &mut usize,
    ) -> Result<Self, Error> {
        let payload_end = *offset + remaining_len;
        let topic_name = read_str(buf, offset)?;

        let (qos, pid) = match header.qos {
            QoS::AtMostOnce => (QoS::AtMostOnce, None),
            QoS::AtLeastOnce => (QoS::AtLeastOnce, Some(Pid::from_buffer(buf, offset)?)),
            QoS::ExactlyOnce => (QoS::ExactlyOnce, Some(Pid::from_buffer(buf, offset)?)),
        };

        Ok(Publish {
            dup: header.dup,
            qos,
            pid,
            retain: header.retain,
            topic_name,
            payload: &buf[*offset..payload_end],
        })
    }

    pub(crate) fn len(&self) -> usize {
        // Length: topic (2+len) + pid (0/2) + payload (len)
        2 + self.topic_name.len()
            + match self.qos {
                QoS::AtMostOnce => 0,
                _ => 2,
            }
            + self.payload.len()
    }

    pub(crate) fn to_buffer(&self, buf: &mut [u8], offset: &mut usize) -> Result<usize, Error> {
        // Header
        let mut header: u8 = match self.qos {
            QoS::AtMostOnce => 0b00110000,
            QoS::AtLeastOnce => 0b00110010,
            QoS::ExactlyOnce => 0b00110100,
        };
        if self.dup {
            header |= 0b00001000 as u8;
        };
        if self.retain {
            header |= 0b00000001 as u8;
        };
        check_remaining(buf, offset, 1)?;
        write_u8(buf, offset, header)?;

        let length = self.len();
        let write_len = write_length(buf, offset, length)? + 1;

        // Topic
        write_string(buf, offset, self.topic_name)?;

        // Pid to be overwritten later on
        match self.qos {
            QoS::AtMostOnce => (),
            QoS::AtLeastOnce => {
                write_u16(buf, offset, 0)?;
            }
            QoS::ExactlyOnce => {
                write_u16(buf, offset, 0)?;
            }
        }

        // Payload
        for &byte in self.payload {
            write_u8(buf, offset, byte)?;
        }

        Ok(write_len)
    }
}
