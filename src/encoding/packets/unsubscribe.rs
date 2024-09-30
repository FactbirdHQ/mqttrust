use crate::{
    encoder::MAX_MQTT_HEADER_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        FixedHeader, PacketType, Pid,
    },
    varint_len, Properties,
};

/// Represents an `UNSUBSCRIBE` packet as defined in the MQTT specification.
///
/// This packet is used to unsubscribe from one or more topics.
///
/// See [MQTT v3.1.1 specification](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398536887)
/// and [MQTT v5 specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453734767)
/// for more details.
#[derive(Debug, Clone, PartialEq)]
pub struct Unsubscribe<'a> {
    /// The packet identifier of the `UNSUBSCRIBE` packet.
    ///
    /// This is only used for QoS 1 and 2.
    pub pid: Option<Pid>,

    /// A list of properties associated with the unsubscribe.
    ///
    /// This is only available in MQTT v5.
    ///
    /// See [MQTT v5 specification](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc453734767)
    /// for more details.
    #[cfg(feature = "mqttv5")]
    pub properties: Properties<'a>,

    /// The list of topics to unsubscribe from.
    pub topics: &'a [&'a str],
}

impl<'a> FixedHeader for Unsubscribe<'a> {
    const PACKET_TYPE: PacketType = PacketType::Unsubscribe;

    /// Returns the flags associated with the fixed header.
    ///
    /// Bits 3,2,1 and 0 of the fixed header of the SUBSCRIBE Control Packet are reserved and MUST be set to 0,0,1 and 0 respectively
    fn flags(&self) -> u8 {
        0b0010
    }
}

impl<'a> Unsubscribe<'a> {
    /// Creates a new `Unsubscribe` packet with the given list of topics.
    pub fn new(topics: &'a [&'a str]) -> Self {
        Self {
            pid: None,
            topics,
            #[cfg(feature = "mqttv5")]
            properties: Properties::Slice(&[]),
        }
    }

    /// Returns the packet identifier of the `UNSUBSCRIBE` packet.
    pub fn pid(&self) -> Option<Pid> {
        self.pid
    }
}

impl<'a> MqttEncode for Unsubscribe<'a> {
    /// Encodes the `UNSUBSCRIBE` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_u16(self.pid.unwrap_or_default().get())?;

        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        for topic in self.topics {
            encoder.write_str(topic)?;
        }

        encoder.finalize_fixed_header(self)?;

        Ok(())
    }

    /// Sets the packet identifier of the `UNSUBSCRIBE` packet.
    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }

    /// Returns the maximum size of the packet in bytes.
    fn max_packet_size(&self) -> usize {
        let mut length = 2 + MAX_MQTT_HEADER_LEN;

        #[cfg(feature = "mqttv5")]
        {
            length += varint_len(self.properties.size());
        }

        length + self.topics.iter().map(|t| t.len() + 2).sum::<usize>()
    }
}
