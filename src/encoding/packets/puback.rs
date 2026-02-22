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
use crate::{encoding::reason_code::PubAckReasonCode, Properties};

use super::PacketType;

/// A PubAck packet is sent by the receiver of a PUBLISH packet to acknowledge
/// that the PUBLISH packet has been received.
///
/// The PubAck packet is used with QoS level 1 and QoS level 2 messages to
/// ensure that the sender of the PUBLISH packet knows that the message
/// has been received.
///
/// [MQTT 3.1.1 spec](https://www.hivemq.com/mqtt-essentials/mqtt-protocol/mqtt-3-1-1-specification/#232-puback-packet)
///
/// [MQTT 5.0 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc394052000)
#[derive(Debug, Clone, PartialEq)]
pub struct PubAck<'a> {
    /// The packet identifier of the PUBLISH packet that is being acknowledged.
    ///
    /// The packet identifier is a unique identifier that is used to identify
    /// a particular PUBLISH packet.
    ///
    /// [MQTT 3.1.1 spec](https://www.hivemq.com/mqtt-essentials/mqtt-protocol/mqtt-3-1-1-specification/#232-puback-packet)
    ///
    /// [MQTT 5.0 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc394052000)
    pub pid: Pid,

    /// The reason code for the PUBACK packet.
    ///
    /// The reason code is used to indicate the reason why the PUBLISH packet
    /// was acknowledged.
    ///
    /// [MQTT 5.0 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc394052000)
    #[cfg(feature = "mqttv5")]
    pub reason_code: PubAckReasonCode,

    /// The properties for the PUBACK packet.
    ///
    /// The properties are used to provide additional information about the
    /// PUBACK packet.
    ///
    /// [MQTT 5.0 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc394052000)
    #[cfg(feature = "mqttv5")]
    pub properties: Properties<'a>,

    #[cfg(feature = "mqttv3")]
    pub(crate) _marker: core::marker::PhantomData<&'a ()>,
}

impl FixedHeader for PubAck<'_> {
    const PACKET_TYPE: PacketType = PacketType::PubAck;
}

impl MqttEncode for PubAck<'_> {
    /// Encodes the `PUBACK` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_u16(self.pid.get())?;

        #[cfg(feature = "mqttv5")]
        if self.reason_code != PubAckReasonCode::Success || self.properties.size() != 0 {
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
        if self.reason_code != PubAckReasonCode::Success || self.properties.size() != 0 {
            // Reason Code
            length += 1;

            // Properties
            length += crate::varint_len(self.properties.size());
        }
        length
    }
}

impl<'a> MqttDecode<'a> for PubAck<'a> {
    /// Decodes the `PUBACK` packet from the given decoder.
    // Branches differ under mqttv5 (reason_code + properties are decoded in the else arm).
    #[allow(clippy::if_same_then_else)]
    fn from_decoder(decoder: &mut crate::decoder::MqttDecoder<'a>) -> Result<Self, Error> {
        if decoder.fixed_header().remaining_len == 2 || decoder.fixed_header().remaining_len == 3 {
            Ok(Self {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubAckReasonCode::Success,
                #[cfg(feature = "mqttv5")]
                properties: Properties::DataBlock(&[]),
                #[cfg(feature = "mqttv3")]
                _marker: core::marker::PhantomData,
            })
        } else {
            Ok(Self {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubAckReasonCode::from(decoder.read_u8()?),
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
    fn test_puback_encode_decode_v311() {
        let puback = PubAck {
            pid: Pid::try_from(1234).unwrap(),
            _marker: core::marker::PhantomData,
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, puback);
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    fn test_puback_encode_decode_v5() {
        let puback = PubAck {
            pid: Pid::try_from(1234).unwrap(),
            reason_code: PubAckReasonCode::Success,
            properties: Properties::default(),
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, puback);
    }

    #[cfg(feature = "mqttv3")]
    #[test]
    fn encode_puback_v311() {
        let expected_bytes = &[0x40, 0x02, 0x00, 0x0C];

        let puback = PubAck {
            pid: Pid::try_from(12).unwrap(),
            _marker: core::marker::PhantomData,
        };

        let mut buf = [0u8; 32];
        let mut encoder = MqttEncoder::new(&mut buf);
        puback.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_puback_v5() {
        let expected_bytes = &[0x40, 0x02, 0x00, 0x0C];

        let puback = PubAck {
            pid: Pid::try_from(12).unwrap(),
            reason_code: PubAckReasonCode::Success,
            properties: Properties::Slice(&[]),
        };

        let mut buf = [0u8; 32];
        let mut encoder = MqttEncoder::new(&mut buf);
        puback.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }
}
