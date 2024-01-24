use crate::{
    encoder::{TxHeader, MAX_MQTT_HEADER_LEN, TX_HEADER_LEN},
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        utils::Pid,
        FixedHeader,
    },
};

use super::PacketType;

pub struct PubAck {
    pub pid: Pid,
}

impl FixedHeader for PubAck {
    const PACKET_TYPE: PacketType = PacketType::PubAck;
}

impl MqttEncode for PubAck {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<TxHeader, Error> {
        encoder.write_u16(self.pid.get())?;
        encoder.finalize_fixed_header(self)?;
        Ok(encoder.write_tx_header(Self::PACKET_TYPE, self.get_qos(), Some(self.pid))?)
    }

    fn max_packet_size(&self) -> usize {
        2 + MAX_MQTT_HEADER_LEN + TX_HEADER_LEN
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

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }
}
