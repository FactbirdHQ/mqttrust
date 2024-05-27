use crate::{
    encoder::{TxHeader, MAX_MQTT_HEADER_LEN, TX_HEADER_LEN},
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        FixedHeader, PacketType, Pid,
    },
    varint_len, Properties,
};

/// Unsubscribe packet ([MQTT 3.10]).
///
/// [MQTT 3.10]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718072
#[derive(Debug, Clone, PartialEq)]
pub struct Unsubscribe<'a> {
    pub pid: Option<Pid>,

    #[cfg(feature = "mqttv5")]
    pub properties: Properties<'a>,

    pub topics: &'a [&'a str],
}

impl<'a> FixedHeader for Unsubscribe<'a> {
    const PACKET_TYPE: PacketType = PacketType::Unsubscribe;

    fn flags(&self) -> u8 {
        // Bits 3,2,1 and 0 of the fixed header of the SUBSCRIBE Control Packet are reserved and MUST be set to 0,0,1 and 0 respectively
        0b0010
    }
}

impl<'a> Unsubscribe<'a> {
    pub fn new(topics: &'a [&'a str]) -> Self {
        Self {
            pid: None,
            topics,
            #[cfg(feature = "mqttv5")]
            properties: Properties::Slice(&[]),
        }
    }

    pub fn pid(&self) -> Option<Pid> {
        self.pid
    }
}

impl<'a> MqttEncode for Unsubscribe<'a> {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<TxHeader, Error> {
        encoder.write_u16(self.pid.unwrap_or_default().get())?;

        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        for topic in self.topics {
            encoder.write_str(topic)?;
        }

        encoder.finalize_fixed_header(self)?;

        Ok(encoder.write_tx_header(Self::PACKET_TYPE, self.get_qos(), self.pid)?)
    }

    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }

    fn max_packet_size(&self) -> usize {
        let mut length = 2 + MAX_MQTT_HEADER_LEN + TX_HEADER_LEN;

        #[cfg(feature = "mqttv5")]
        {
            length += varint_len(self.properties.size());
        }

        length + self.topics.iter().map(|t| t.len() + 2).sum::<usize>()
    }
}
