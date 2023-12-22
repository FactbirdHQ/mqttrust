use super::*;

/// https://docs.solace.com/MQTT-311-Prtl-Conformance-Spec/MQTT%20Control%20Packets.htm#_Toc430864901
const FIXED_HEADER_LEN: usize = 5;
const PID_LEN: usize = 2;

// /// Base enum for all MQTT packet types.
// ///
// /// This is the main type you'll be interacting with, as an output of
// /// [`decode_slice()`] and the input to [`encode()`]. Most variants can be
// /// constructed directly without using methods.
// ///
// /// ```
// /// # use mqttrust::encoding::v4::*;
// /// # use core::convert::TryFrom;
// /// // Simplest form
// /// let pkt = Packet::Connack(Connack { session_present: false,
// ///                                     code: ConnectReturnCode::Accepted });
// /// // Using `Into` trait
// /// let publish = Publish { dup: false,
// ///                         qos: QoS::AtMostOnce,
// ///                         retain: false,
// ///                         pid: None,
// ///                         topic_name: "to/pic",
// ///                         payload: b"payload" };
// /// let pkt: Packet = publish.into();
// /// // Identifyer-only packets
// /// let pkt = Packet::Puback(Pid::try_from(42).unwrap());
// /// ```
// ///
// /// [`encode()`]: fn.encode.html
// /// [`decode_slice()`]: fn.decode_slice.html
// #[derive(Debug, Clone, PartialEq)]
// pub enum Packet<'a> {
//     /// [MQTT 3.1](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718028)
//     Connect(Connect<'a>),
//     /// [MQTT 3.2](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718033)
//     ConnAck(ConnAck<'a>),
//     /// [MQTT 3.3](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718037)
//     Publish(Publish<'a>),
//     /// [MQTT 3.4](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718043)
//     Puback(Pid),
//     /// [MQTT 3.5](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718048)
//     Pubrec(Pid),
//     /// [MQTT 3.6](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718053)
//     Pubrel(Pid),
//     /// [MQTT 3.7](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718058)
//     Pubcomp(Pid),
//     /// [MQTT 3.8](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718063)
//     Subscribe(Subscribe<'a>),
//     /// [MQTT 3.9](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718068)
//     Suback(Suback<'a>),
//     /// [MQTT 3.10](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718072)
//     Unsubscribe(Unsubscribe<'a>),
//     /// [MQTT 3.11](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718077)
//     Unsuback(Pid),
//     /// [MQTT 3.12](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718081)
//     Pingreq,
//     /// [MQTT 3.13](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718086)
//     Pingresp,
//     /// [MQTT 3.14](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718090)
//     Disconnect,
// }

// impl<'a> Packet<'a> {
//     /// Return the packet type variant.
//     ///
//     /// This can be used for matching, categorising, debuging, etc. Most users will match directly
//     /// on `Packet` instead.
//     pub fn get_type(&self) -> PacketType {
//         match self {
//             Packet::Connect(_) => PacketType::Connect,
//             Packet::ConnAck(_) => PacketType::ConnAck,
//             Packet::Publish(_) => PacketType::Publish,
//             Packet::Puback(_) => PacketType::PubAck,
//             Packet::Pubrec(_) => PacketType::PubRec,
//             Packet::Pubrel(_) => PacketType::PubRel,
//             Packet::Pubcomp(_) => PacketType::PubComp,
//             Packet::Subscribe(_) => PacketType::Subscribe,
//             Packet::Suback(_) => PacketType::SubAck,
//             Packet::Unsubscribe(_) => PacketType::Unsubscribe,
//             Packet::Unsuback(_) => PacketType::UnsubAck,
//             Packet::Pingreq => PacketType::PingReq,
//             Packet::Pingresp => PacketType::PingResp,
//             Packet::Disconnect => PacketType::Disconnect,
//         }
//     }

//     pub(crate) async fn write<'b>(
//         &self,
//         network: &mut Network<'b, impl embedded_nal_async::TcpConnect>,
//     ) -> Result<(), utils::StateError> {

//         //FIXME: 
//         let mut buf = [0u8; 128];
//         let mut encoder = MqttEncoder::new(&mut buf);
//         encoder.encode_packet(self).map_err(|_| utils::StateError::Deserialization)?;

//         info!("{:?}", &encoder.bytes());
//         network.write(encoder.bytes()).await
//     }
// }

// macro_rules! packet_from_borrowed {
//     ($($t:ident),+) => {
//         $(
//             impl<'a> From<$t<'a>> for Packet<'a> {
//                 fn from(p: $t<'a>) -> Self {
//                     Packet::$t(p)
//                 }
//             }
//         )+
//     }
// }

// packet_from_borrowed!(Suback, Connect, Publish, Subscribe, Unsubscribe, ConnAck);

/// Packet type variant, without the associated data.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum PacketType {
    Connect = 0x10,
    ConnAck = 0x20,
    Publish = 0x30,
    PubAck = 0x40,
    PubRec = 0x50,
    PubRel = 0x62,
    PubComp = 0x70,
    Subscribe = 0x82,
    SubAck = 0x90,
    Unsubscribe = 0xA2,
    UnsubAck = 0xB0,
    PingReq = 0xC0,
    PingResp = 0xD0,
    Disconnect = 0xE0,
    Auth = 0xF0,
}

impl TryFrom<u8> for PacketType {
    type Error = Error;

    fn try_from(orig: u8) -> Result<Self, Self::Error> {
        let packet_type: u8 = orig & 0xF0;
        match packet_type {
            0x10 => Ok(PacketType::Connect),
            0x20 => Ok(PacketType::ConnAck),
            0x30 => Ok(PacketType::Publish),
            0x40 => Ok(PacketType::PubAck),
            0x50 => Ok(PacketType::PubRec),
            0x60 => Ok(PacketType::PubRel),
            0x70 => Ok(PacketType::PubComp),
            0x80 => Ok(PacketType::Subscribe),
            0x90 => Ok(PacketType::SubAck),
            0xA0 => Ok(PacketType::Unsubscribe),
            0xB0 => Ok(PacketType::UnsubAck),
            0xC0 => Ok(PacketType::PingReq),
            0xD0 => Ok(PacketType::PingResp),
            0xE0 => Ok(PacketType::Disconnect),
            0xF0 => Ok(PacketType::Auth),
            _ => Err(Error::InvalidHeader)
        }
    }
}
