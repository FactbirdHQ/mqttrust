#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![feature(async_fn_in_trait)]
#![feature(type_alias_impl_trait)]

use num_enum::TryFromPrimitive;
/// Perfect scenario:
/// - Once subscribe() is called with a topic, that subscription is guaranteed to be kept until a corresponding unsubscribe(), also across reconnects
/// - Subscribe, publish, unsubscribe can be awaited with following guarantees:
///     - QoS0: Future doesn't return Ready until package has been sent (Left the local network buffer) 
///     - QoS1: Future doesn't return Ready until package has been ack'ed
/// - Client can somehow receive messages
///     Even better if it can only receive for topics it has subscribed to
/// - If built with spooling (persistance), the client can choose on a per-publish basis if it should be buffered? (check PAHO if this is standard)
/// - Client probably needs to be `&mut`, with a client() fn on the event runner
/// - Support MQTTv5 and optionally MQTTv3
/// 
/// 
/// 
/// Overall design is very similar to embassy networking stacks, where an MQTT client ≃ socket
///
/// - `MPSCChannel` needs be basically a framed version of an MPSC Queue, preferably wrapped in an MQTT Packet serializer
/// - `Channel` needs to a framed version of `Pipe`, preferably wrapped in an MQTT Packet serializer




mod de;
mod ser;
mod error;
mod runner;
mod message_types;
mod packets;
mod varint;
mod reason_codes;
mod will;
mod types;
mod publication;
mod properties;
mod ring_buffer;
mod exp;

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


// pub trait AsyncMqtt {
//     async fn send(&mut self, packet: Packet<'_>) -> Result<(), MqttError>;

//     async fn publish(&mut self, topic_name: &str, payload: &[u8], qos: QoS) -> Result<(), MqttError> {
//         let packet = Packet::Publish(Publish {
//             dup: false,
//             qos,
//             pid: None,
//             retain: false,
//             topic_name,
//             payload,
//         });

//         self.send(packet)
//     }

//     async fn subscribe(&mut self, topics: &[SubscribeTopic<'_>]) -> Result<(), MqttError> {
//         let packet = Packet::Subscribe(Subscribe::new(topics));
//         self.send(packet)
//     }

//     async fn unsubscribe(&mut self, topics: &[&str]) -> Result<(), MqttError> {
//         let packet = Packet::Unsubscribe(Unsubscribe::new(topics));
//         self.send(packet)
//     }
// }

// MAX_SUBS = Prov (TMP) + IO (PUB) + OTA (TMP) + DD (PUB) + Shadows (Status [PUB], Wifi, Sensors) + Jobs
