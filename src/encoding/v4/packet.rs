use super::*;

/// https://docs.solace.com/MQTT-311-Prtl-Conformance-Spec/MQTT%20Control%20Packets.htm#_Toc430864901
const FIXED_HEADER_LEN: usize = 5;
const PID_LEN: usize = 2;

/// Base enum for all MQTT packet types.
///
/// This is the main type you'll be interacting with, as an output of [`decode_slice()`] and an input of
/// [`encode()`]. Most variants can be constructed directly without using methods.
///
/// ```
/// # use mqttrust::encoding::v4::*;
/// # use core::convert::TryFrom;
/// // Simplest form
/// let pkt = Packet::Connack(Connack { session_present: false,
///                                     code: ConnectReturnCode::Accepted });
/// // Using `Into` trait
/// let publish = Publish { dup: false,
///                         qos: QoS::AtMostOnce,
///                         retain: false,
///                         pid: None,
///                         topic_name: "to/pic",
///                         payload: b"payload" };
/// let pkt: Packet = publish.into();
/// // Identifyer-only packets
/// let pkt = Packet::Puback(Pid::try_from(42).unwrap());
/// ```
///
/// [`encode()`]: fn.encode.html
/// [`decode_slice()`]: fn.decode_slice.html
#[derive(Debug, Clone, PartialEq)]
pub enum Packet<'a> {
    /// [MQTT 3.1](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718028)
    Connect(Connect<'a>),
    /// [MQTT 3.2](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718033)
    Connack(Connack),
    /// [MQTT 3.3](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718037)
    Publish(Publish<'a>),
    /// [MQTT 3.4](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718043)
    Puback(Pid),
    /// [MQTT 3.5](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718048)
    Pubrec(Pid),
    /// [MQTT 3.6](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718053)
    Pubrel(Pid),
    /// [MQTT 3.7](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718058)
    Pubcomp(Pid),
    /// [MQTT 3.8](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718063)
    Subscribe(Subscribe<'a>),
    /// [MQTT 3.9](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718068)
    Suback(Suback<'a>),
    /// [MQTT 3.10](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718072)
    Unsubscribe(Unsubscribe<'a>),
    /// [MQTT 3.11](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718077)
    Unsuback(Pid),
    /// [MQTT 3.12](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718081)
    Pingreq,
    /// [MQTT 3.13](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718086)
    Pingresp,
    /// [MQTT 3.14](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718090)
    Disconnect,
}

impl<'a> Packet<'a> {
    /// Return the packet type variant.
    ///
    /// This can be used for matching, categorising, debuging, etc. Most users will match directly
    /// on `Packet` instead.
    pub fn get_type(&self) -> PacketType {
        match self {
            Packet::Connect(_) => PacketType::Connect,
            Packet::Connack(_) => PacketType::Connack,
            Packet::Publish(_) => PacketType::Publish,
            Packet::Puback(_) => PacketType::Puback,
            Packet::Pubrec(_) => PacketType::Pubrec,
            Packet::Pubrel(_) => PacketType::Pubrel,
            Packet::Pubcomp(_) => PacketType::Pubcomp,
            Packet::Subscribe(_) => PacketType::Subscribe,
            Packet::Suback(_) => PacketType::Suback,
            Packet::Unsubscribe(_) => PacketType::Unsubscribe,
            Packet::Unsuback(_) => PacketType::Unsuback,
            Packet::Pingreq => PacketType::Pingreq,
            Packet::Pingresp => PacketType::Pingresp,
            Packet::Disconnect => PacketType::Disconnect,
        }
    }

    pub fn len(&self) -> usize {
        let variable_len = match self {
            Packet::Connect(c) => c.len(),
            Packet::Connack(_) => 2,
            Packet::Publish(p) => p.len(),
            Packet::Puback(_)
            | Packet::Pubrec(_)
            | Packet::Pubrel(_)
            | Packet::Pubcomp(_)
            | Packet::Unsuback(_) => PID_LEN,
            Packet::Suback(_) => PID_LEN + 1,
            Packet::Subscribe(s) => s.len(),
            Packet::Unsubscribe(u) => u.len(),
            Packet::Pingreq | Packet::Pingresp | Packet::Disconnect => 0,
        };

        FIXED_HEADER_LEN + variable_len
    }
}

macro_rules! packet_from_borrowed {
    ($($t:ident),+) => {
        $(
            impl<'a> From<$t<'a>> for Packet<'a> {
                fn from(p: $t<'a>) -> Self {
                    Packet::$t(p)
                }
            }
        )+
    }
}
macro_rules! packet_from {
    ($($t:ident),+) => {
        $(
            impl<'a> From<$t> for Packet<'a> {
                fn from(p: $t) -> Self {
                    Packet::$t(p)
                }
            }
        )+
    }
}

packet_from_borrowed!(Suback, Connect, Publish, Subscribe, Unsubscribe);
packet_from!(Connack);

/// Packet type variant, without the associated data.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt-impl", derive(defmt::Format))]
pub enum PacketType {
    Connect,
    Connack,
    Publish,
    Puback,
    Pubrec,
    Pubrel,
    Pubcomp,
    Subscribe,
    Suback,
    Unsubscribe,
    Unsuback,
    Pingreq,
    Pingresp,
    Disconnect,
}
