use crate::encoding::{
    encoder::{MqttEncode, MqttEncoder},
    error::Error,
    FixedHeader,
};

use super::PacketType;

pub struct PingReq;

impl FixedHeader for PingReq {
    const PACKET_TYPE: PacketType = PacketType::PingReq;

    fn variable_header_len(&self) -> usize {
        0
    }
}

impl MqttEncode for PingReq {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_fixed_header(self)?;
        Ok(())
    }
}
