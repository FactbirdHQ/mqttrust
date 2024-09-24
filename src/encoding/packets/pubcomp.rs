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

pub struct PubComp {
    pub pid: Pid,
}

impl FixedHeader for PubComp {
    const PACKET_TYPE: PacketType = PacketType::PubComp;
}

impl MqttEncode for PubComp {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<TxHeader, Error> {
        encoder.write_u16(self.pid.get())?;
        encoder.finalize_fixed_header(self)?;
        Ok(encoder.write_tx_header(Some(self.pid))?)
    }

    fn max_packet_size(&self) -> usize {
        2 + MAX_MQTT_HEADER_LEN + TX_HEADER_LEN
    }
}
