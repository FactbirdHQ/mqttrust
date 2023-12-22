use crate::encoding::{
    encoder::{MqttEncode, MqttEncoder},
    error::Error,
    FixedHeader, PacketType, Pid,
};

/// Unsubscribe packet ([MQTT 3.10]).
///
/// [MQTT 3.10]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718072
#[derive(Debug, Clone, PartialEq)]
pub struct Unsubscribe<'a> {
    pub pid: Option<Pid>,
    pub topics: &'a [&'a str],
}

impl<'a> FixedHeader for Unsubscribe<'a> {
    const PACKET_TYPE: PacketType = PacketType::Unsubscribe;

    fn remaining_len(&self) -> usize {
        2 + self.topics.iter().map(|t| t.len() + 2).sum::<usize>()
    }
}

impl<'a> Unsubscribe<'a> {
    pub fn new(topics: &'a [&'a str]) -> Self {
        Self { pid: None, topics }
    }

    pub fn pid(&self) -> Option<Pid> {
        self.pid
    }
}

impl<'a> MqttEncode for Unsubscribe<'a> {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_fixed_header(self)?;

        encoder.write_u16(self.pid.unwrap_or_default().get())?;

        for topic in self.topics {
            encoder.write_str(topic)?;
        }
        Ok(())
    }

    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }
}
