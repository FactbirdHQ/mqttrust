use crate::{
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

    fn variable_header_len(&self) -> usize {
        2 + varint_len(self.properties.size())
    }

    fn payload_len(&self) -> usize {
        self.topics.iter().map(|t| t.len() + 2).sum::<usize>()
    }

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
            properties: Properties::Slice(&[]),
        }
    }

    pub fn pid(&self) -> Option<Pid> {
        self.pid
    }
}

impl<'a> MqttEncode for Unsubscribe<'a> {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_fixed_header(self)?;

        encoder.write_u16(self.pid.unwrap_or_default().get())?;

        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        for topic in self.topics {
            encoder.write_str(topic)?;
        }

        Ok(())
    }

    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }
}
