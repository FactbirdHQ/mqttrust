use crate::{
    decoder::MqttDecode,
    encoder::{MqttEncode, MqttEncoder, MAX_MQTT_HEADER_LEN},
    encoding::reason_code::ConnAckReasonCode,
    varint_len, EncodingError as Error, FixedHeader,
};

#[cfg(feature = "mqttv5")]
use crate::Properties;

use super::PacketType;

/// Represents the MQTT CONNACK packet, which is sent by the broker in response to a CONNECT packet.
///
/// The CONNACK packet contains information about the connection establishment, including whether the
/// session is being maintained, the success status of the connection, and optional properties.
///
/// **See:**
/// * [MQTT v3.1.1 specification, section 3.2.2](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc345374721)
/// * [MQTT v5 specification, section 4.2.1](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453733186)
#[derive(Debug, Clone, PartialEq)]
pub struct ConnAck<'a> {
    /// Indicates true if session state is being maintained by the broker.
    ///
    /// **See:**
    /// * [MQTT v3.1.1 specification, section 3.2.2.1](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc345374737)
    /// * [MQTT v5 specification, section 4.2.1.2](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453733199)
    pub(crate) session_present: bool,

    /// A status code indicating the success status of the connection.
    ///
    /// **See:**
    /// * [MQTT v3.1.1 specification, section 3.2.2.2](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc345374743)
    /// * [MQTT v5 specification, section 4.2.1.3](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453733205)
    pub(crate) reason_code: ConnAckReasonCode,

    /// A list of properties associated with the connection.
    ///
    /// This field is only present in MQTT v5.
    ///
    /// **See:**
    /// * [MQTT v5 specification, section 4.2.1.4](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453733211)
    #[cfg(feature = "mqttv5")]
    pub(crate) properties: Properties<'a>,

    #[cfg(feature = "mqttv3")]
    _marker: core::marker::PhantomData<&'a ()>,
}

impl FixedHeader for ConnAck<'_> {
    const PACKET_TYPE: PacketType = PacketType::ConnAck;
}

impl MqttEncode for ConnAck<'_> {
    /// Encodes the `CONNACK` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_u8(self.session_present as u8)?;
        encoder.write_u8(self.reason_code as u8)?;

        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        encoder.finalize_fixed_header(self)?;
        Ok(())
    }

    /// Returns the maximum size of the packet in bytes.
    fn max_packet_size(&self) -> usize {
        let mut length = 2 + MAX_MQTT_HEADER_LEN;

        #[cfg(feature = "mqttv5")]
        {
            length += varint_len(self.properties.size());
        }

        length
    }
}

impl<'a> MqttDecode<'a> for ConnAck<'a> {
    /// Decodes the `CONNACK` packet from the given decoder.
    fn from_decoder(decoder: &mut crate::decoder::MqttDecoder<'a>) -> Result<Self, Error> {
        let conn_ack_flags = decoder.read_u8()?;

        if conn_ack_flags & 0b11111110 != 0 {
            return Err(Error::MalformedPacket);
        }

        Ok(Self {
            session_present: (conn_ack_flags & 0b1 == 1),
            reason_code: ConnAckReasonCode::from(decoder.read_u8()?),
            #[cfg(feature = "mqttv5")]
            properties: decoder.read_properties()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_roundtrip;

    #[test]
    #[cfg(feature = "mqttv3")]
    fn test_connack_encode_decode_v311() {
        use core::marker::PhantomData;

        let connack = ConnAck {
            session_present: true,
            reason_code: ConnAckReasonCode::Success,
            _marker: PhantomData,
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, connack);
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    fn test_connack_encode_decode_v5() {
        use crate::Property;

        let properties = Properties::Slice(&[Property::SessionExpiryInterval(100)]);

        let connack = ConnAck {
            session_present: false,
            reason_code: ConnAckReasonCode::Success,
            #[cfg(feature = "mqttv5")]
            properties,
        };

        let mut buf = [0u8; 512];
        let mut encoder = MqttEncoder::new(&mut buf);

        test_roundtrip(&mut encoder, connack);
    }
}
