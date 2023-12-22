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

pub use error::Error;
pub use error::StateError;

pub use packets::*;
pub use properties::*;
pub use utils::*;

pub trait FixedHeader {
    const PACKET_TYPE: packets::PacketType;

    fn remaining_len(&self) -> usize;

    fn flags(&self) -> u8 {
        0
    }

    fn packet_type(&self) -> u8 {
        Self::PACKET_TYPE.into()
    }

    fn packet_len(&self) -> usize {
        2 + self.remaining_len()
    }
}
