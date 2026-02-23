mod connack;
mod connect;
mod disconnect;
mod pingreq;
mod pingresp;
mod puback;
#[cfg(feature = "qos2")]
mod pubcomp;
mod publish;
#[cfg(feature = "qos2")]
mod pubrec;
#[cfg(feature = "qos2")]
mod pubrel;
mod suback;
mod subscribe;
mod unsuback;
mod unsubscribe;

pub use connack::*;
pub use connect::*;
pub use disconnect::*;
use num_enum::{IntoPrimitive, TryFromPrimitive};
pub use pingreq::*;
pub use pingresp::*;
pub use puback::*;
#[cfg(feature = "qos2")]
pub use pubcomp::*;
pub use publish::*;
#[cfg(feature = "qos2")]
pub use pubrec::*;
#[cfg(feature = "qos2")]
pub use pubrel::*;
pub use suback::*;
pub use subscribe::*;
pub use unsuback::*;
pub use unsubscribe::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PacketType {
    Connect = 0x10,
    ConnAck = 0x20,
    Publish = 0x30,
    PubAck = 0x40,
    #[cfg(feature = "qos2")]
    PubRec = 0x50,
    #[cfg(feature = "qos2")]
    PubRel = 0x60,
    #[cfg(feature = "qos2")]
    PubComp = 0x70,
    Subscribe = 0x80,
    SubAck = 0x90,
    Unsubscribe = 0xA0,
    UnsubAck = 0xB0,
    PingReq = 0xC0,
    PingResp = 0xD0,
    Disconnect = 0xE0,
    Auth = 0xF0,
}

#[cfg(test)]
pub mod tests {
    use crate::{
        decoder::MqttDecode,
        encoder::{MqttEncode, MqttEncoder},
    };

    /// Helper function to test encode/decode roundtrip for various packet types.
    pub fn test_roundtrip<
        'a,
        T: MqttEncode + MqttDecode<'a> + core::fmt::Debug + core::cmp::PartialEq,
    >(
        encoder: &'a mut MqttEncoder,
        packet: T,
    ) {
        // Encode the packet
        packet.to_buffer(encoder).unwrap();

        let encoded = encoder.packet_bytes();

        let mut decoder = crate::decoder::MqttDecoder::try_new(encoded).unwrap();

        // Decode the packet
        let decoded_packet = T::from_decoder(&mut decoder).unwrap();

        // Assert that the encoded and decoded packets are equal
        assert_eq!(packet, decoded_packet);
    }
}
