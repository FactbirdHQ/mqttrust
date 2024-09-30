use bon::{builder, Builder};

use crate::{
    decoder::MqttDecode,
    encoder::MAX_MQTT_HEADER_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        reason_code::DisconnectReasonCode,
        FixedHeader,
    },
    varint_len, Properties,
};

use super::PacketType;

/// Represents a `DISCONNECT` packet as defined in the MQTT specification.
///
/// This packet is used to close the network connection between a client and a server.
///
/// See [MQTT v3.1.1 specification](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398536887)
/// and [MQTT v5 specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453734767)
/// for more details.
#[derive(Debug, Clone, PartialEq, Builder)]
pub struct Disconnect<'a> {
    /// A status code indicating the success status of the connection.
    ///
    /// See [MQTT v3.1.1 specification](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398536887)
    /// and [MQTT v5 specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453734767)
    /// for more details.
    #[builder(default = DisconnectReasonCode::Normal)]
    pub(crate) reason_code: DisconnectReasonCode,

    /// A list of properties associated with the disconnect.
    ///
    /// This is only available in MQTT v5.
    ///
    /// See [MQTT v5 specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453734767)
    /// for more details.
    #[cfg(feature = "mqttv5")]
    #[builder(default = Properties::Slice(&[]))]
    pub(crate) properties: Properties<'a>,

    #[cfg(feature = "mqttv3")]
    #[builder(skip = core::marker::PhantomData)]
    pub(crate) _marker: core::marker::PhantomData<&'a ()>,
}

impl<'a> FixedHeader for Disconnect<'a> {
    const PACKET_TYPE: PacketType = PacketType::Disconnect;
}

impl<'a> MqttEncode for Disconnect<'a> {
    /// Encodes the `DISCONNECT` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        if self.reason_code != DisconnectReasonCode::Normal {
            encoder.write_u8(self.reason_code as u8)?;
        }

        #[cfg(feature = "mqttv5")]
        {
            if self.properties.size() != 0 && self.reason_code == DisconnectReasonCode::Normal {
                encoder.write_u8(self.reason_code as u8)?;
            }
            encoder.write_properties(&self.properties)?;
        }

        encoder.finalize_fixed_header(self)?;
        Ok(())
    }

    /// Returns the maximum size of the packet in bytes.
    fn max_packet_size(&self) -> usize {
        let mut length = 0;

        if self.reason_code != DisconnectReasonCode::Normal {
            length += 1;
        }

        #[cfg(feature = "mqttv5")]
        {
            if self.properties.size() != 0 && self.reason_code == DisconnectReasonCode::Normal {
                length += 1;
            }
            length += varint_len(self.properties.size());
            length += self.properties.size();
        }

        length + MAX_MQTT_HEADER_LEN
    }
}

impl<'a> MqttDecode<'a> for Disconnect<'a> {
    /// Decodes the `DISCONNECT` packet from the given decoder.
    fn from_decoder(decoder: &mut crate::decoder::MqttDecoder<'a>) -> Result<Self, Error> {
        if decoder.fixed_header().remaining_len < 2 {
            Ok(Self::builder().build())
        } else {
            Ok(Self {
                reason_code: DisconnectReasonCode::from(decoder.read_u8()?),
                #[cfg(feature = "mqttv5")]
                properties: decoder.read_properties()?,
                #[cfg(feature = "mqttv3")]
                _marker: PhantomData,
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
    fn test_disconnect_encode_decode_v311() {
        use core::marker::PhantomData;

        let disconnect = Disconnect::builder().build();

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, disconnect);
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    fn test_disconnect_encode_decode_v5() {
        use crate::Property;
        let properties = Properties::Slice(&[Property::SessionExpiryInterval(100)]);
        let disconnect = Disconnect::builder().properties(properties).build();

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, disconnect);
    }
}
