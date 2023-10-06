#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![feature(async_fn_in_trait)]
#![feature(type_alias_impl_trait)]

use num_enum::TryFromPrimitive;


pub mod de;
pub mod ser;
pub mod error;
pub mod message_types;
pub mod packets;
pub mod varint;
pub mod reason_codes;
pub mod will;
pub mod types;
pub mod publication;
pub mod properties;
pub mod ring_buffer;
pub mod exp;
pub mod pubsub;

pub use properties::Property;


 /// The quality-of-service for an MQTT message.
 #[derive(Debug, Copy, Clone, PartialEq, TryFromPrimitive, PartialOrd)]
 #[repr(u8)]
 pub enum QoS {
     /// A packet will be delivered at most once, but may not be delivered at all.
     AtMostOnce = 0,
 
     /// A packet will be delivered at least one time, but possibly more than once.
     AtLeastOnce = 1,
 
     /// A packet will be delivered exactly one time.
     ExactlyOnce = 2,
 }
 
 /// The retained status for an MQTT message.
 #[derive(Debug, Copy, Clone, PartialEq, TryFromPrimitive)]
 #[repr(u8)]
 pub enum Retain {
     /// The message shall not be retained by the broker.
     NotRetained = 0,
 
     /// The message shall be marked for retention by the broker.
     Retained = 1,
 }
