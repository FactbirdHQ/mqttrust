use crate::{
    decoder::MqttDecode,
    encoder::MAX_MQTT_HEADER_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        FixedHeader,
    },
};

use super::PacketType;

/// Represents a `PINGRESP` packet as defined in the MQTT specification.
///
/// This packet is the response to a `PINGREQ` packet. It indicates that the network connection
/// between a client and a server is still alive.
///
/// See [MQTT v3.1.1 specification](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398536887)
/// and [MQTT v5 specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453734767)
/// for more details.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PingResp;

impl FixedHeader for PingResp {
    const PACKET_TYPE: PacketType = PacketType::PingResp;
}

impl MqttEncode for PingResp {
    /// Encodes the `PINGRESP` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.finalize_fixed_header(self)?;
        Ok(())
    }

    /// Returns the maximum size of the packet in bytes.
    fn max_packet_size(&self) -> usize {
        MAX_MQTT_HEADER_LEN
    }
}

impl<'a> MqttDecode<'a> for PingResp {
    /// Decodes the `PINGRESP` packet from the given decoder.
    fn from_decoder(_decoder: &mut crate::decoder::MqttDecoder<'a>) -> Result<Self, Error> {
        Ok(Self)
    }
}
