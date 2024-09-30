use crate::{
    encoder::MAX_MQTT_HEADER_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        utils::Pid,
        FixedHeader,
    },
};

#[cfg(feature = "mqttv5")]
use crate::{encoding::reason_code::PubRecReasonCode, Properties};

use super::PacketType;

/// PUBREC packet - Publish Received.
///
/// The PUBREC packet is sent by the Server to the Client in response to a PUBLISH packet with QoS 2.
///
/// ## MQTT Specification References
///
/// * [MQTT v5.0](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc385372107): Section 4.7
#[derive(Debug, Clone, PartialEq)]
pub struct PubRec<'a> {
    /// The Packet Identifier of the original PUBLISH packet.
    ///
    /// * [MQTT v5.0](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc385372107): Section 4.7.1
    pub(crate) pid: Pid,

    /// The reason code of the PUBREC packet.
    ///
    /// * [MQTT v5.0](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc385372107): Section 4.7.2
    #[cfg(feature = "mqttv5")]
    pub(crate) reason_code: PubRecReasonCode,

    /// The optional Properties of the PUBREC packet.
    ///
    /// * [MQTT v5.0](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc385372107): Section 4.7.3
    #[cfg(feature = "mqttv5")]
    pub(crate) properties: Properties<'a>,

    #[cfg(feature = "mqttv3")]
    pub(crate) _marker: core::marker::PhantomData<&'a ()>,
}

impl FixedHeader for PubRec {
    const PACKET_TYPE: PacketType = PacketType::PubRec;
}

impl MqttEncode for PubRec {
    /// Encodes the `PUBREC` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_u16(self.pid.get())?;

        #[cfg(feature = "mqttv5")]
        if self.reason_code != PubRecReasonCode::Success || self.properties.size() != 0 {
            encoder.write_u8(self.reason_code as u8)?;
            encoder.write_properties(&self.properties)?;
        }

        encoder.finalize_fixed_header(self)?;
        Ok(())
    }

    /// Returns the maximum size of the packet in bytes.
    fn max_packet_size(&self) -> usize {
        let mut length = 2 + MAX_MQTT_HEADER_LEN;

        #[cfg(feature = "mqttv5")]
        if self.reason_code != PubRecReasonCode::Success || self.properties.size() != 0 {
            // Reason Code
            length += 1;

            // Properties
            length += crate::varint_len(self.properties.size());
        }

        length
    }
}

impl<'a> MqttDecode<'a> for PubRec<'a> {
    /// Decodes the `PUBREC` packet from the given decoder.
    fn from_decoder(decoder: &mut crate::decoder::MqttDecoder<'a>) -> Result<Self, Error> {
        if decoder.fixed_header().remaining_len == 2 || decoder.fixed_header().remaining_len == 3 {
            Ok(Self {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubRecReasonCode::Success,
                #[cfg(feature = "mqttv5")]
                properties: Properties::DataBlock(&[]),
                #[cfg(feature = "mqttv3")]
                _marker: core::marker::PhantomData,
            })
        } else {
            Ok(Self {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubRecReasonCode::from(decoder.read_u8()?),
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
    fn test_pubrec_encode_decode_v311() {
        let pubrec = PubRec {
            pid: Pid::new(1234),
            _marker: core::marker::PhantomData,
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, pubrec);
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    fn test_pubrec_encode_decode_v5() {
        let pubrec = PubRec {
            pid: Pid::new(1234),
            reason_code: PubRecReasonCode::Success,
            properties: Properties::default(),
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, pubrec);
    }
}
