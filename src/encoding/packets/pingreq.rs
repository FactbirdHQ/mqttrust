use crate::{
    encoder::MAX_MQTT_HEADER_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        FixedHeader,
    },
};

use super::PacketType;

pub struct PingReq;

impl FixedHeader for PingReq {
    const PACKET_TYPE: PacketType = PacketType::PingReq;
}

impl MqttEncode for PingReq {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.finalize_fixed_header(self)?;
        Ok(())
    }

    fn max_packet_size(&self) -> usize {
        MAX_MQTT_HEADER_LEN
    }
}
