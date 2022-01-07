use mqttrust::encoding::v4::{
    decoder::{read_header, Header},
    packet::PacketType,
    utils::{Pid, QoS},
};

use crate::state::StateError;

pub struct SerializedPacket<'a>(pub &'a mut [u8]);

impl<'a> SerializedPacket<'a> {
    pub fn header(&self) -> Result<Header, StateError> {
        Header::new(self.0[0]).map_err(|_| StateError::InvalidHeader)
    }

    pub fn set_pid(&mut self, pid: Pid) -> Result<(), StateError> {
        let mut offset = 0;
        let (header, _) = read_header(self.0, &mut offset)
            .map_err(|_| StateError::InvalidHeader)?
            .ok_or(StateError::InvalidHeader)?;

        match (header.typ, header.qos) {
            (PacketType::Publish, QoS::AtLeastOnce | QoS::ExactlyOnce) => {
                if self.0[offset..].len() < 2 {
                    return Err(StateError::InvalidHeader);
                }
                let len = ((self.0[offset] as usize) << 8) | self.0[offset + 1] as usize;

                offset += 2;
                if len > self.0[offset..].len() {
                    return Err(StateError::InvalidHeader);
                } else {
                    offset += len;
                }
            }
            (
                PacketType::Subscribe
                | PacketType::Unsubscribe
                | PacketType::Suback
                | PacketType::Puback
                | PacketType::Pubrec
                | PacketType::Pubrel
                | PacketType::Pubcomp
                | PacketType::Unsuback,
                _,
            ) => {}
            _ => return Ok(()),
        }

        pid.to_buffer(&mut self.0, &mut offset)
            .map_err(|_| StateError::PidMissing)
    }

    pub fn to_inner(self) -> &'a mut [u8] {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use core::convert::TryFrom;

    use mqttrust::{
        encoding::v4::{decode_slice, encode_slice},
        Packet, Publish, Subscribe, SubscribeTopic,
    };

    use super::*;

    #[test]
    fn set_publish_pid() {
        let publish = Packet::Publish(Publish {
            dup: false,
            qos: QoS::ExactlyOnce,
            pid: None,
            retain: false,
            topic_name: "test",
            payload: b"Whatup",
        });

        let buf = &mut [0u8; 2048];
        let len = encode_slice(&publish, buf).unwrap();

        let mut ser_packet = SerializedPacket(&mut buf[..len]);

        let header = ser_packet.header().unwrap();

        assert_eq!(header.typ, PacketType::Publish);
        assert_eq!(header.qos, QoS::ExactlyOnce);

        ser_packet.set_pid(Pid::try_from(54).unwrap()).unwrap();

        let p = decode_slice(ser_packet.to_inner()).unwrap();

        assert_eq!(
            p,
            Some(Packet::Publish(Publish {
                dup: false,
                qos: QoS::ExactlyOnce,
                pid: Some(Pid::try_from(54).unwrap()),
                retain: false,
                topic_name: "test",
                payload: b"Whatup",
            }))
        )
    }

    #[test]
    fn set_subscribe_pid() {
        let subscribe = Packet::Subscribe(Subscribe::new(&[SubscribeTopic {
            topic_path: "AWESOME",
            qos: QoS::AtLeastOnce,
        }]));

        let buf = &mut [0u8; 2048];
        let len = encode_slice(&subscribe, buf).unwrap();

        let mut ser_packet = SerializedPacket(&mut buf[..len]);

        let header = ser_packet.header().unwrap();

        assert_eq!(header.typ, PacketType::Subscribe);
        assert_eq!(header.qos, QoS::AtLeastOnce);

        ser_packet.set_pid(Pid::try_from(65).unwrap()).unwrap();

        let p = decode_slice(ser_packet.to_inner()).unwrap();

        match p {
            Some(Packet::Subscribe(p)) => {
                assert_eq!(p.pid(), Some(Pid::try_from(65).unwrap()));
                assert_eq!(
                    p.topics().next(),
                    Some(SubscribeTopic {
                        topic_path: "AWESOME",
                        qos: QoS::AtLeastOnce,
                    })
                );
            }
            _ => panic!(),
        }
    }
}
