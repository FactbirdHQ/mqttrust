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
pub use reason_code::*;
pub use utils::*;

/// A trait for representing the fixed header of an MQTT packet.
pub trait FixedHeader {
    /// The packet type of the fixed header.
    const PACKET_TYPE: packets::PacketType;

    /// Returns the flags associated with the fixed header.
    ///
    /// The default implementation returns 0, meaning no flags are set.
    fn flags(&self) -> u8 {
        0
    }

    /// Returns the packet type as a byte.
    fn packet_type(&self) -> u8 {
        Self::PACKET_TYPE.into()
    }
}

/// Calculates the length of a variable length integer (varint).
///
/// This function takes the integer value and returns the number of bytes required to encode it.
pub fn varint_len(len: usize) -> usize {
    match len {
        0..=127 => 1,
        128..=16383 => 2,
        16384..=2097151 => 3,
        2097152..=268435455 => 4,
        _ => panic!("Varint length larger than supported ({} > 268435455)", len),
    }
}
