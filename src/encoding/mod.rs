#[cfg(all(feature = "mqttv3", feature = "mqttv5"))]
compile_error!("You must enable at most one of the following features: mqttv3, mqttv5");

pub(crate) mod decoder;
pub(crate) mod encoder;
mod error;
mod packets;
mod properties;
mod reason_code;
pub(crate) mod received_packet;
mod utils;

pub use error::Error as EncodingError;
pub use error::StateError;

pub use packets::*;
pub use properties::*;
pub use utils::*;

pub trait FixedHeader {
    const PACKET_TYPE: packets::PacketType;

    fn flags(&self) -> u8 {
        0
    }

    fn packet_type(&self) -> u8 {
        Self::PACKET_TYPE.into()
    }
}

pub fn varint_len(len: usize) -> usize {
    match len {
        0..=127 => 1,
        128..=16383 => 2,
        16384..=2097151 => 3,
        2097152..=268435455 => 4,
        _ => panic!("Varint length larger than supported ({} > 268435455)", len),
    }
}
