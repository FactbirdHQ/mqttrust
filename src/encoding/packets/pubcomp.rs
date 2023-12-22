use crate::encoding::{
    encoder::{MqttEncode, MqttEncoder},
    error::Error,
    utils::Pid,
    FixedHeader,
};

use super::PacketType;

pub struct PubComp {
    pub pid: Pid,
}

impl FixedHeader for PubComp {
    const PACKET_TYPE: PacketType = PacketType::PubComp;

    fn remaining_len(&self) -> usize {
        2
    }
}

impl MqttEncode for PubComp {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_fixed_header(self)?;
        encoder.write_u16(self.pid.get())?;
        Ok(())
    }
}
