use crate::{
    decoder::MqttDecode,
    encoder::MAX_MQTT_HEADER_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        utils::Pid,
        FixedHeader,
    },
};

#[cfg(feature = "mqttv5")]
use crate::varint_len;

#[cfg(feature = "mqttv5")]
use crate::Properties;

use super::PacketType;

/// UnsubAck packet - Unsubscribe Acknowledgement.
///
/// This packet is sent by the Server to the Client in response to an UNSUBSCRIBE packet.
///
/// ## MQTT Specification References
///
/// * [MQTT v3.1.1](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc385372116): Section 4.6
/// * [MQTT v5.0](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc385372092): Section 4.6
#[derive(Debug, Clone, PartialEq)]
pub struct UnsubAck<'a> {
    /// The Packet Identifier as sent in the UNSUBSCRIBE packet.
    ///
    /// * [MQTT v3.1.1](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc385372116): Section 4.6.1
    /// * [MQTT v5.0](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc385372092): Section 4.6.1
    pub(crate) pid: Pid,

    #[cfg(feature = "mqttv5")]
    /// The optional Properties of the Unsubscribe Acknowledgement packet.
    ///
    /// * [MQTT v5.0](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc385372092): Section 4.6.3
    pub(crate) properties: Properties<'a>,

    /// The response status code of the subscription request.
    ///
    /// * [MQTT v3.1.1](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc385372116): Section 4.6.2
    /// * [MQTT v5.0](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc385372092): Section 4.6.2
    pub(crate) codes: &'a [u8],
}

impl FixedHeader for UnsubAck<'_> {
    const PACKET_TYPE: PacketType = PacketType::UnsubAck;
}

impl MqttEncode for UnsubAck<'_> {
    /// Encodes the `UNSUBACK` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_u16(self.pid.get())?;
        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        #[cfg(feature = "mqttv5")]
        encoder.write_payload(&self.codes)?;

        encoder.finalize_fixed_header(self)?;
        Ok(())
    }

    /// Returns the maximum size of the packet in bytes.
    fn max_packet_size(&self) -> usize {
        #[allow(unused_mut)]
        let mut length = 2 + MAX_MQTT_HEADER_LEN;
        #[cfg(feature = "mqttv5")]
        {
            length += varint_len(self.properties.size());
            length += self.codes.len();
        }
        length
    }
}

impl<'a> MqttDecode<'a> for UnsubAck<'a> {
    /// Decodes the `UNSUBACK` packet from the given decoder.
    fn from_decoder(decoder: &mut crate::decoder::MqttDecoder<'a>) -> Result<Self, Error> {
        Ok(Self {
            pid: Pid::try_from(decoder.read_u16()?)?,
            #[cfg(feature = "mqttv5")]
            properties: decoder.read_properties()?,
            codes: decoder.read_payload()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_roundtrip;

    #[test]
    #[cfg(feature = "mqttv3")]
    fn test_unsuback_encode_decode_v311() {
        let unsuback = UnsubAck {
            pid: Pid::try_from(1234).unwrap(),
            codes: &[],
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, unsuback);
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    fn test_unsuback_encode_decode_v5() {
        let unsuback = UnsubAck {
            pid: Pid::try_from(1234).unwrap(),
            codes: &[
                0,
                0,
                // UnsubAckReturnCode::Success as u8,
                // UnsubAckReturnCode::Success as u8,
            ],
            #[cfg(feature = "mqttv5")]
            properties: Properties::default(),
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, unsuback);
    }
}
