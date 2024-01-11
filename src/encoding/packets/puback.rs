use crate::encoding::{
    encoder::{MqttEncode, MqttEncoder},
    error::Error,
    utils::Pid,
    FixedHeader,
};

use super::PacketType;

pub struct PubAck {
    pub pid: Pid,
}

impl FixedHeader for PubAck {
    const PACKET_TYPE: PacketType = PacketType::PubAck;

    fn variable_header_len(&self) -> usize {
        2
    }
}

impl MqttEncode for PubAck {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_fixed_header(self)?;
        encoder.write_u16(self.pid.get())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_puback_v5() {
        let expected_bytes = &[0x40, 0x02, 0x00, 0x0C];

        let puback = PubAck {
            pid: Pid::try_from(12).unwrap(),
        };

        let mut buf = [0u8; 32];
        let mut encoder = MqttEncoder::new(&mut buf);
        puback.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.bytes(), expected_bytes);
    }
}
