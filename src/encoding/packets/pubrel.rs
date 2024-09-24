use crate::encoding::{
    encoder::{MqttEncode, MqttEncoder},
    error::Error,
    utils::Pid,
    FixedHeader,
};

use super::PacketType;

pub struct PubRel {
    pub pid: Pid,
}

impl FixedHeader for PubRel {
    const PACKET_TYPE: PacketType = PacketType::PubRel;

    fn remaining_len(&self) -> usize {
        2
    }
}

impl MqttEncode for PubRel {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<TxHeader, Error> {
        encoder.write_u16(self.pid.get())?;
        encoder.finalize_fixed_header(self)?;
        Ok(encoder.write_tx_header(Some(self.pid))?)
    }
}
