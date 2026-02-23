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
use crate::{encoding::reason_code::PubCompReasonCode, Properties};

use super::PacketType;

// Represents a `PUBCOMP` packet as defined in the MQTT specification.
///
/// This packet is the response to a `PUBREL` packet. It acknowledges the successful reception
/// of the `PUBREL` packet and completes the QoS 2 publish flow.
///
/// See [MQTT v3.1.1 specification](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398536887)
/// and [MQTT v5 specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453734767)
/// for more details.
#[derive(Debug, PartialEq)]
pub struct PubComp<'a> {
    /// The packet identifier of the `PUBREL` packet.
    pub(crate) pid: Pid,

    /// The properties and success status of associated with the publication.
    #[cfg(feature = "mqttv5")]
    pub(crate) reason_code: PubCompReasonCode,

    /// The properties associated with the publication.
    #[cfg(feature = "mqttv5")]
    pub(crate) properties: Properties<'a>,

    #[cfg(feature = "mqttv3")]
    pub(crate) _marker: core::marker::PhantomData<&'a ()>,
}

impl FixedHeader for PubComp<'_> {
    const PACKET_TYPE: PacketType = PacketType::PubComp;
}

impl MqttEncode for PubComp<'_> {
    /// Encodes the `PUBCOMP` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_u16(self.pid.get())?;

        #[cfg(feature = "mqttv5")]
        if self.reason_code != PubCompReasonCode::Success || self.properties.size() != 0 {
            encoder.write_u8(self.reason_code as u8)?;
            encoder.write_properties(&self.properties)?;
        }

        encoder.finalize_fixed_header(self)?;
        Ok(())
    }

    /// Returns the maximum size of the packet in bytes.
    fn max_packet_size(&self) -> usize {
        #[allow(unused_mut)]
        let mut length = 2 + MAX_MQTT_HEADER_LEN;
        #[cfg(feature = "mqttv5")]
        if self.reason_code != PubCompReasonCode::Success || self.properties.size() != 0 {
            // Reason Code
            length += 1;

            // Properties
            length += crate::varint_len(self.properties.size());
        }
        length
    }
}

impl<'a> MqttDecode<'a> for PubComp<'a> {
    /// Decodes the `PUBCOMP` packet from the given decoder.
    // Branches differ under mqttv5 (reason_code + properties are decoded in the else arm).
    #[allow(clippy::if_same_then_else)]
    fn from_decoder(decoder: &mut crate::decoder::MqttDecoder<'a>) -> Result<Self, Error> {
        if decoder.fixed_header().remaining_len == 2 || decoder.fixed_header().remaining_len == 3 {
            Ok(Self {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubCompReasonCode::Success,
                #[cfg(feature = "mqttv5")]
                properties: Properties::DataBlock(&[]),
                #[cfg(feature = "mqttv3")]
                _marker: core::marker::PhantomData,
            })
        } else {
            Ok(Self {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubCompReasonCode::from(decoder.read_u8()?),
                #[cfg(feature = "mqttv5")]
                properties: decoder.read_properties()?,
                #[cfg(feature = "mqttv3")]
                _marker: core::marker::PhantomData,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_roundtrip;

    #[test]
    #[cfg(feature = "mqttv3")]
    fn test_pubcomp_encode_decode_v311() {
        let pubcomp = PubComp {
            pid: Pid::try_from(1234).unwrap(),
            _marker: core::marker::PhantomData,
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, pubcomp);
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    fn test_pubcomp_encode_decode_v5() {
        let pubcomp = PubComp {
            pid: Pid::try_from(1234).unwrap(),
            reason_code: PubCompReasonCode::Success,
            properties: Properties::default(),
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, pubcomp);
    }
}
