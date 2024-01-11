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

    fn variable_header_len(&self) -> usize;

    fn flags(&self) -> u8 {
        0
    }

    fn packet_type(&self) -> u8 {
        Self::PACKET_TYPE.into()
    }

    fn payload_len(&self) -> usize {
        0
    }

    fn remaining_len(&self) -> usize {
        self.variable_header_len() + self.payload_len()
    }

    fn packet_len(&self) -> usize {
        let len = self.variable_header_len() + self.payload_len();

        1 + varint_len(len)
    }
}

pub fn varint_len(len: usize) -> usize {
    match len {
        0..=127 => len + 1,
        128..=16383 => len + 2,
        16384..=2097151 => len + 3,
        2097152..=268435455 => len + 4,
        _ => panic!("Remaining length larger than supported"),
    }
}
