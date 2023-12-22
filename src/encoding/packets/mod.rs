mod connect;
mod pingreq;
mod puback;
#[cfg(feature = "qos2")]
mod pubcomp;
mod publish;
#[cfg(feature = "qos2")]
mod pubrec;
#[cfg(feature = "qos2")]
mod pubrel;
mod subscribe;
mod unsubscribe;

pub use connect::*;
use num_enum::{IntoPrimitive, TryFromPrimitive};
pub use pingreq::*;
pub use puback::*;
#[cfg(feature = "qos2")]
pub use pubcomp::*;
pub use publish::*;
#[cfg(feature = "qos2")]
pub use pubrec::*;
#[cfg(feature = "qos2")]
pub use pubrel::*;
pub use subscribe::*;
pub use unsubscribe::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
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
