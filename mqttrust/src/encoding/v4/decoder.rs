use super::*;

pub fn decode_slice(buf: &[u8]) -> Result<Option<Packet<'_>>, Error> {
    let mut offset = 0;
    if let Some((header, remaining_len)) = read_header(buf, &mut offset)? {
        let r = read_packet(header, remaining_len, buf, &mut offset)?;
        Ok(Some(r))
    } else {
        // Don't have a full packet
        Ok(None)
    }
}

fn read_packet<'a>(
    header: Header,
    remaining_len: usize,
    buf: &'a [u8],
    offset: &mut usize,
) -> Result<Packet<'a>, Error> {
    Ok(match header.typ {
        PacketType::Pingreq => Packet::Pingreq,
        PacketType::Pingresp => Packet::Pingresp,
        PacketType::Disconnect => Packet::Disconnect,
        PacketType::Connect => Connect::from_buffer(buf, offset)?.into(),
        PacketType::Connack => Connack::from_buffer(buf, offset)?.into(),
        PacketType::Publish => Publish::from_buffer(&header, remaining_len, buf, offset)?.into(),
        PacketType::Puback => Packet::Puback(Pid::from_buffer(buf, offset)?),
        PacketType::Pubrec => Packet::Pubrec(Pid::from_buffer(buf, offset)?),
        PacketType::Pubrel => Packet::Pubrel(Pid::from_buffer(buf, offset)?),
        PacketType::Pubcomp => Packet::Pubcomp(Pid::from_buffer(buf, offset)?),
        PacketType::Subscribe => Subscribe::from_buffer(remaining_len, buf, offset)?.into(),
        PacketType::Suback => Suback::from_buffer(remaining_len, buf, offset)?.into(),
        PacketType::Unsubscribe => Unsubscribe::from_buffer(remaining_len, buf, offset)?.into(),
        PacketType::Unsuback => Packet::Unsuback(Pid::from_buffer(buf, offset)?),
    })
}

/// Read the parsed header and remaining_len from the buffer. Only return Some() and advance the
/// buffer position if there is enough data in the buffer to read the full packet.
pub fn read_header(buf: &[u8], offset: &mut usize) -> Result<Option<(Header, usize)>, Error> {
    let mut len: usize = 0;
    for pos in 0..=3 {
        if buf.len() > *offset + pos + 1 {
            let byte = buf[*offset + pos + 1];
            len += (byte as usize & 0x7F) << (pos * 7);
            if (byte & 0x80) == 0 {
                // Continuation bit == 0, length is parsed
                if buf.len() < *offset + 2 + pos + len {
                    // Won't be able to read full packet
                    return Ok(None);
                }
                // Parse header byte, skip past the header, and return
                let header = Header::new(buf[*offset])?;
                *offset += pos + 2;
                return Ok(Some((header, len)));
            }
        } else {
            // Couldn't read full length
            return Ok(None);
        }
    }
    // Continuation byte == 1 four times, that's illegal.
    Err(Error::InvalidHeader)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub typ: PacketType,
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
}
impl Header {
    pub fn new(hd: u8) -> Result<Header, Error> {
        let (typ, flags_ok) = match hd >> 4 {
            1 => (PacketType::Connect, hd & 0b1111 == 0),
            2 => (PacketType::Connack, hd & 0b1111 == 0),
            3 => (PacketType::Publish, true),
            4 => (PacketType::Puback, hd & 0b1111 == 0),
            5 => (PacketType::Pubrec, hd & 0b1111 == 0),
            6 => (PacketType::Pubrel, hd & 0b1111 == 0b0010),
            7 => (PacketType::Pubcomp, hd & 0b1111 == 0),
            8 => (PacketType::Subscribe, hd & 0b1111 == 0b0010),
            9 => (PacketType::Suback, hd & 0b1111 == 0),
            10 => (PacketType::Unsubscribe, hd & 0b1111 == 0b0010),
            11 => (PacketType::Unsuback, hd & 0b1111 == 0),
            12 => (PacketType::Pingreq, hd & 0b1111 == 0),
            13 => (PacketType::Pingresp, hd & 0b1111 == 0),
            14 => (PacketType::Disconnect, hd & 0b1111 == 0),
            _ => (PacketType::Connect, false),
        };
        if !flags_ok {
            return Err(Error::InvalidHeader);
        }
        Ok(Header {
            typ,
            dup: hd & 0b1000 != 0,
            qos: QoS::from_u8((hd & 0b110) >> 1)?,
            retain: hd & 1 == 1,
        })
    }
}

pub(crate) fn read_str<'a>(buf: &'a [u8], offset: &mut usize) -> Result<&'a str, Error> {
    core::str::from_utf8(read_bytes(buf, offset)?).map_err(|_| Error::InvalidString)
}

pub(crate) fn read_bytes<'a>(buf: &'a [u8], offset: &mut usize) -> Result<&'a [u8], Error> {
    if buf[*offset..].len() < 2 {
        return Err(Error::InvalidLength);
    }
    let len = ((buf[*offset] as usize) << 8) | buf[*offset + 1] as usize;

    *offset += 2;
    if len > buf[*offset..].len() {
        Err(Error::InvalidLength)
    } else {
        let bytes = &buf[*offset..*offset + len];
        *offset += len;
        Ok(bytes)
    }
}
