use crate::{
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

pub struct Disconnect<'a> {
    /// A status code indicating the success status of the connection.
    pub reason_code: DisconnectReasonCode,

    /// A list of properties associated with the disconnect.
    #[cfg(feature = "mqttv5")]
    pub properties: Properties<'a>,

    #[cfg(feature = "mqttv3")]
    pub _marker: core::marker::PhantomData<&'a ()>,
}

impl<'a> FixedHeader for Disconnect<'a> {
    const PACKET_TYPE: PacketType = PacketType::Disconnect;
}

impl<'a> MqttEncode for Disconnect<'a> {
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
