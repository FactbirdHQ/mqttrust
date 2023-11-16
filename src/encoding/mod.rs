#[cfg_attr(feature = "mqttv3", path = "v4/mod.rs")]
#[cfg_attr(feature = "mqttv5", path = "v5/mod.rs")]
pub mod version;

pub(crate) mod decoder;
pub(crate) mod encoder;
pub(crate) mod packet;
pub(crate) mod utils;

use core::str::FromStr as _;

pub use decoder::decode_slice;
pub use encoder::encode_slice;
pub use packet::{Packet, PacketType};
pub use utils::{Error, Pid, QoS, QosPid};
pub use version::*;

/// Protocol version.
///
/// Sent in [`Connect`] packet.
///
/// [`Connect`]: struct.Connect.html
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// [MQTT 3.1.1] is the most commonly implemented version.
    ///
    /// [MQTT 3.1.1]: https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html
    MQTT311,
    /// MQIsdp, aka SCADA are pre-standardisation names of MQTT. It should mostly conform to MQTT
    /// 3.1.1, but you should watch out for implementation discrepancies.
    MQIsdp,
    /// [MQTT 5]: https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html
    MQTT5,
}
impl Protocol {
    pub(crate) fn new(name: &str, level: u8) -> Result<Protocol, Error> {
        match (name, level) {
            ("MQIsdp", 3) => Ok(Protocol::MQIsdp),
            ("MQTT", 4) => Ok(Protocol::MQTT311),
            ("MQTT", 5) => Ok(Protocol::MQTT5),
            _ => Err(Error::InvalidProtocol(
                heapless::String::from_str(name).unwrap(),
                level,
            )),
        }
    }

    pub(crate) fn from_buffer(buf: &[u8], offset: &mut usize) -> Result<Self, Error> {
        let protocol_name = decoder::read_str(buf, offset)?;
        let protocol_level = buf[*offset];
        *offset += 1;

        Protocol::new(protocol_name, protocol_level)
    }

    pub(crate) fn to_buffer(&self, buf: &mut [u8], offset: &mut usize) -> Result<usize, Error> {
        match self {
            Protocol::MQTT311 => {
                let slice = &[0u8, 4, b'M', b'Q', b'T', b'T', 4];
                for &byte in slice {
                    encoder::write_u8(buf, offset, byte)?;
                }
                Ok(slice.len())
            }
            Protocol::MQIsdp => {
                let slice = &[0u8, 4, b'M', b'Q', b'i', b's', b'd', b'p', 4];
                for &byte in slice {
                    encoder::write_u8(buf, offset, byte)?;
                }
                Ok(slice.len())
            }
            Protocol::MQTT5 => {
                let slice = &[0u8, 4, b'M', b'Q', b'T', b'T', 5];
                for &byte in slice {
                    encoder::write_u8(buf, offset, byte)?;
                }
                Ok(slice.len())
            }
        }
    }
}
