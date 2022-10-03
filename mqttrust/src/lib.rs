#![cfg_attr(not(test), no_std)]

// This mod MUST go first, so that the others see its macros.
pub(crate) mod fmt;

pub mod encoding;

pub use encoding::v4::{
    subscribe::SubscribeTopic, utils::QoS, Packet, Publish, Subscribe, Unsubscribe,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt-impl", derive(defmt::Format))]
pub enum MqttError {
    /// Queue full, cannot send/receive more packets
    Full,
    /// RefCell borrow fault
    Borrow,
    /// Needed resource is unavailable
    Unavailable,
}

pub trait Mqtt {
    fn send(&self, packet: Packet<'_>) -> Result<(), MqttError>;

    fn client_id(&self) -> &str;

    fn publish(&self, topic_name: &str, payload: &[u8], qos: QoS) -> Result<(), MqttError> {
        let packet = Packet::Publish(Publish {
            dup: false,
            qos,
            pid: None,
            retain: false,
            topic_name,
            payload,
        });

        self.send(packet)
    }

    fn subscribe(&self, topics: &[SubscribeTopic<'_>]) -> Result<(), MqttError> {
        let packet = Packet::Subscribe(Subscribe::new(topics));
        self.send(packet)
    }

    fn unsubscribe(&self, topics: &[&str]) -> Result<(), MqttError> {
        let packet = Packet::Unsubscribe(Unsubscribe::new(topics));
        self.send(packet)
    }
}
