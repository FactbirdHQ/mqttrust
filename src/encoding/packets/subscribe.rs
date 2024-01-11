use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::{
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        FixedHeader, PacketType, Pid, QoS,
    },
    varint_len,
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

    fn variable_header_len(&self) -> usize {
        2 + varint_len(self.properties.size())
    }

    fn payload_len(&self) -> usize {
        self.topics
            .iter()
            .map(|t| 2 + t.topic_path.len() + 1)
            .sum::<usize>()
    }

    fn flags(&self) -> u8 {
        // Bits 3,2,1 and 0 of the fixed header of the SUBSCRIBE Control Packet are reserved and MUST be set to 0,0,1 and 0 respectively
        0b0010
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
                options_byte |= u8::from(topic.retain_handling) << 4;
                options_byte |= u8::from(topic.retain_as_published) << 3;
                options_byte |= u8::from(topic.no_local) << 2;
            }

            options_byte |= u8::from(topic.maximum_qos) & 0b0000_0011;

            encoder.write_u8(options_byte)?;
        }

        Ok(())
    }

    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_subscribe_v5() {
        let expected_bytes = [
            0x82, 0x0e, 0x00, 0x01, 0x00, 0x00, 0x08, 0x74, 0x65, 0x73, 0x74, 0x2f, 0x31, 0x32,
            0x33, 0x00,
        ];

        let sub = Subscribe {
            pid: Some(Pid::new()),
            properties: Properties::Slice(&[]),
            topics: &[SubscribeTopic {
                topic_path: "test/123",
                maximum_qos: QoS::AtMostOnce,
                no_local: false,
                retain_as_published: false,
                retain_handling: RetainHandling::SendAtSubscribeTime,
            }],
        };

        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        sub.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.bytes(), expected_bytes);
    }
}
