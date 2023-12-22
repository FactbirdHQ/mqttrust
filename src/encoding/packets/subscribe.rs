use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::encoding::{
    encoder::{MqttEncode, MqttEncoder},
    error::Error,
    FixedHeader, PacketType, Pid, QoS,
};

#[cfg(feature = "mqttv5")]
use crate::Properties;

#[cfg(feature = "mqttv5")]
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, TryFromPrimitive, IntoPrimitive)]
pub enum RetainHandling {
    SendAtSubscribeTime = 0,
    SendAtSubscribeTimeIfNonexistent = 1,
    DoNotSend = 2,
}

/// Subscribe topic.
///
/// [Subscribe] packets contain a `Vec` of those.
///
/// [Subscribe]: struct.Subscribe.html
#[derive(Debug, Clone, PartialEq)]
pub struct SubscribeTopic<'a> {
    pub topic_path: &'a str,
    pub maximum_qos: QoS,
    #[cfg(feature = "mqttv5")]
    pub no_local: bool,
    #[cfg(feature = "mqttv5")]
    pub retain_as_published: bool,
    #[cfg(feature = "mqttv5")]
    pub retain_handling: RetainHandling,
}

impl<'a> Into<&'a str> for SubscribeTopic<'a> {
    fn into(self) -> &'a str {
        self.topic_path
    }
}

/// Subscribe packet ([MQTT 3.8]).
///
/// [MQTT 3.8]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718063
#[derive(Debug, Clone, PartialEq)]
pub struct Subscribe<'a> {
    pub pid: Option<Pid>,

    #[cfg(feature = "mqttv5")]
    pub properties: Properties<'a>,

    pub topics: &'a [SubscribeTopic<'a>],
}

impl<'a> FixedHeader for Subscribe<'a> {
    const PACKET_TYPE: PacketType = PacketType::Subscribe;

    fn remaining_len(&self) -> usize {
        2 + self
            .topics
            .iter()
            .map(|t| t.topic_path.len() + 3)
            .sum::<usize>()
    }
}

impl<'a> Subscribe<'a> {
    pub fn new(topics: &'a [SubscribeTopic<'a>]) -> Self {
        Self {
            pid: None,
            #[cfg(feature = "mqttv5")]
            properties: Properties::Slice(&[]),
            topics,
        }
    }

    pub fn pid(&self) -> Option<Pid> {
        self.pid
    }
}

impl<'a> MqttEncode for Subscribe<'a> {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_fixed_header(self)?;

        // Pid
        encoder.write_u16(self.pid.unwrap_or_default().get())?;

        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        // Topics
        for topic in self.topics {
            encoder.write_str(topic.topic_path)?;

            let mut options_byte = 0b0000_0000;

            #[cfg(feature = "mqttv5")]
            {
                options_byte |= (u8::from(topic.retain_handling) & 0b0000_0011) << 4;

                if topic.retain_as_published {
                    options_byte |= 0b0000_1000;
                }
            }

            #[cfg(feature = "mqttv5")]
            if topic.no_local {
                options_byte |= 0b0000_0100;
            }

            let qos_byte = topic.maximum_qos as u8;
            options_byte |= qos_byte & 0b0000_0011;

            encoder.write_u8(options_byte)?;
        }

        Ok(())
    }

    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }
}
