use bon::Builder;
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::{
    decoder::MqttDecode,
    encoder::MAX_MQTT_HEADER_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        utils::QoS,
        FixedHeader,
    },
};

#[cfg(feature = "mqttv5")]
use crate::varint_len;

#[cfg(feature = "mqttv5")]
use crate::Properties;

use super::PacketType;

/// Protocol version.
///
/// Sent in [`Connect`] packet.
///
/// [`Connect`]: struct.Connect.html
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Protocol {
    /// MQIsdp, aka SCADA are pre-standardisation names of MQTT. It should mostly conform to MQTT
    /// 3.1.1, but you should watch out for implementation discrepancies.
    MQIsdp = 3,
    /// [MQTT 3.1.1] is the most commonly implemented version.
    ///
    /// [MQTT 3.1.1]: https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html
    MQTT311 = 4,
    /// [MQTT 5]: https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html
    MQTT5 = 5,
}

impl Protocol {
    fn name(&self) -> &str {
        match self {
            Protocol::MQTT311 | Protocol::MQTT5 => "MQTT",
            Protocol::MQIsdp => "MQIsdp",
        }
    }
}

/// Message that the server should publish when the client disconnects.
///
/// Sent by the client in the [Connect] packet. [MQTT 3.1.3.2].
///
/// [Connect]: struct.Connect.html
/// [MQTT 3.1.3.2]: https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc3901060
#[derive(Debug, Clone, PartialEq, Builder)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LastWill<'a> {
    pub(crate) topic: &'a str,
    #[builder(default = &[])]
    pub(crate) data: &'a [u8],
    #[builder(default = QoS::AtLeastOnce)]
    pub(crate) qos: QoS,
    #[builder(default = false)]
    pub(crate) retained: bool,
    #[cfg(feature = "mqttv5")]
    #[builder(default = Properties::Slice(&[]))]
    pub(crate) properties: Properties<'a>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Connect<'a> {
    pub(crate) protocol: Protocol,

    /// Any properties associated with the CONNECT request.
    ///
    /// This field is only present when using MQTT v5.
    ///
    /// [MQTT v5 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc408185329)
    #[cfg(feature = "mqttv5")]
    pub(crate) properties: Properties<'a>,

    /// Specifies the keep-alive interval of the connection in seconds.
    ///
    /// The keep-alive interval is the maximum time interval in seconds that the Client is willing to wait
    /// between receiving control packets from the Server before assuming that the connection is broken.
    ///
    /// This field is required for both MQTT v3.1.1 and MQTT v5.
    ///
    /// [MQTT v3.1.1 spec](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc408983217)
    /// [MQTT v5 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc408185330)
    pub(crate) keep_alive: u16,

    /// The ID of the client that is connecting. May be an empty string to automatically allocate
    /// an ID from the broker.
    ///
    /// This field is required for both MQTT v3.1.1 and MQTT v5.
    ///
    /// [MQTT v3.1.1 spec](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc408983219)
    /// [MQTT v5 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc408185332)
    pub(crate) client_id: &'a str,

    /// An optional authentication message used by the server.
    ///
    /// [MQTT v3.1.1 spec](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc408983220)
    /// [MQTT v5 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc408185333)
    pub(crate) username: Option<&'a str>,
    pub(crate) password: Option<&'a [u8]>,

    /// An optional will message to be transmitted whenever the connection is lost.
    ///
    /// [MQTT v3.1.1 spec](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc408983221)
    /// [MQTT v5 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc408185334)
    pub(crate) last_will: Option<LastWill<'a>>,

    /// Specified true there is no session state being taken in to the MQTT connection.
    ///
    /// This field is required for both MQTT v3.1.1 and MQTT v5.
    ///
    /// [MQTT v3.1.1 spec](https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc408983222)
    /// [MQTT v5 spec](https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc408185335)
    pub(crate) clean_start: bool,
}

impl<'a> FixedHeader for Connect<'a> {
    const PACKET_TYPE: PacketType = PacketType::Connect;
}

impl MqttEncode for Connect<'_> {
    /// Encodes the `CONNECT` packet into the given encoder.
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_str(self.protocol.name())?;
        encoder.write_u8(self.protocol.into())?;

        // TODO: Change `connect_flags` to bitflags
        let mut connect_flags = 0;
        connect_flags |= u8::from(self.clean_start) << 1;
        connect_flags |= u8::from(self.username.is_some()) << 7;
        connect_flags |= u8::from(self.password.is_some()) << 6;
        if let Some(last_will) = &self.last_will {
            connect_flags |= 0b00000100;
            connect_flags |= u8::from(last_will.qos) << 3;
            connect_flags |= u8::from(last_will.retained) << 5;
        };

        encoder.write_u8(connect_flags)?;
        encoder.write_u16(self.keep_alive)?;

        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        encoder.write_str(self.client_id)?;

        if let Some(last_will) = &self.last_will {
            #[cfg(feature = "mqttv5")]
            encoder.write_properties(&last_will.properties)?;

            encoder.write_str(last_will.topic)?;
            encoder.write_slice(last_will.data)?;
        };

        if let Some(username) = self.username {
            encoder.write_str(username)?;
        };
        if let Some(password) = self.password {
            encoder.write_slice(password)?;
        };

        encoder.finalize_fixed_header(self)?;

        Ok(())
    }

    /// Returns the maximum size of the packet in bytes.
    fn max_packet_size(&self) -> usize {
        let mut length: usize = 2 + self.protocol.name().len() + 1 + 1; // NOTE: protocol_name(6) + protocol_level(1) + flags(1);
        length += 2; // keep alive

        #[cfg(feature = "mqttv5")]
        {
            length += varint_len(self.properties.size());
            length += self.properties.size();
        }

        length += 2 + self.client_id.len();
        if let Some(last_will) = &self.last_will {
            #[cfg(feature = "mqttv5")]
            {
                length += varint_len(last_will.properties.size());
                length += last_will.properties.size();
            }
            length += 2 + last_will.topic.len();
            length += 2 + last_will.data.len();
        };
        if let Some(username) = self.username {
            length += 2 + username.len();
        };
        if let Some(password) = self.password {
            length += 2 + password.len();
        };

        length + MAX_MQTT_HEADER_LEN
    }
}

impl<'a> MqttDecode<'a> for Connect<'a> {
    /// Decodes the `CONNECT` packet from the given decoder.
    fn from_decoder(_decoder: &mut crate::decoder::MqttDecoder<'a>) -> Result<Self, Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_connect_v5() {
        let expected_bytes = &[
            16, 17, 0, 4, 77, 81, 84, 84, 5, 2, 0, 60, 0, 0, 4, 84, 69, 83, 84,
        ];

        let connect = Connect {
            protocol: Protocol::MQTT5,
            properties: Properties::Slice(&[]),
            keep_alive: 60,
            client_id: "TEST",
            username: None,
            password: None,
            last_will: None,
            clean_start: true,
        };

        let mut buf = [0u8; 32];
        let mut encoder = MqttEncoder::new(&mut buf);
        connect.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv3")]
    #[test]
    fn encode_connect_v311() {
        let expected_bytes = &[
            0x10, 0x10, 0x00, 0x04, 0x4D, 0x51, 0x54, 0x54, 0x04, 0x02, 0x00, 0x3C, 0x00, 0x04,
            0x54, 0x45, 0x53, 0x54,
        ];

        let connect = Connect {
            protocol: Protocol::MQTT311,
            keep_alive: 60,
            client_id: "TEST",
            username: None,
            password: None,
            last_will: None,
            clean_start: true,
        };

        let mut buf = [0u8; 32];
        let mut encoder = MqttEncoder::new(&mut buf);
        connect.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_connect_v5_username_password() {
        let expected_bytes = &[
            0x10, 0x1D, // CONNECT, remaining length = 29
            0x00, 0x04, 0x4D, 0x51, 0x54, 0x54, // "MQTT"
            0x05, // protocol level 5
            0xC2, // flags: clean_start + username + password
            0x00, 0x3C, // keep alive = 60
            0x00, // connect properties length = 0
            0x00, 0x04, 0x54, 0x45, 0x53, 0x54, // client_id "TEST"
            0x00, 0x04, 0x75, 0x73, 0x65, 0x72, // username "user"
            0x00, 0x04, 0x70, 0x61, 0x73, 0x73, // password "pass"
        ];

        let connect = Connect {
            protocol: Protocol::MQTT5,
            properties: Properties::Slice(&[]),
            keep_alive: 60,
            client_id: "TEST",
            username: Some("user"),
            password: Some(b"pass"),
            last_will: None,
            clean_start: true,
        };

        let mut buf = [0u8; 64];
        let mut encoder = MqttEncoder::new(&mut buf);
        connect.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_connect_v5_last_will() {
        let expected_bytes = &[
            0x10, 0x1B, // CONNECT, remaining length = 27
            0x00, 0x04, 0x4D, 0x51, 0x54, 0x54, // "MQTT"
            0x05, // protocol level 5
            0x0E, // flags: clean_start + will + QoS1
            0x00, 0x3C, // keep alive = 60
            0x00, // connect properties length = 0
            0x00, 0x04, 0x54, 0x45, 0x53, 0x54, // client_id "TEST"
            0x00, // will properties length = 0
            0x00, 0x03, 0x74, 0x2F, 0x77, // will topic "t/w"
            0x00, 0x02, 0x01, 0x02, // will data [0x01, 0x02]
        ];

        let connect = Connect {
            protocol: Protocol::MQTT5,
            properties: Properties::Slice(&[]),
            keep_alive: 60,
            client_id: "TEST",
            username: None,
            password: None,
            last_will: Some(LastWill {
                topic: "t/w",
                data: &[0x01, 0x02],
                qos: QoS::AtLeastOnce,
                retained: false,
                properties: Properties::Slice(&[]),
            }),
            clean_start: true,
        };

        let mut buf = [0u8; 64];
        let mut encoder = MqttEncoder::new(&mut buf);
        connect.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv5")]
    #[test]
    fn encode_connect_v5_all_fields() {
        let expected_bytes = &[
            0x10, 0x27, // CONNECT, remaining length = 39
            0x00, 0x04, 0x4D, 0x51, 0x54, 0x54, // "MQTT"
            0x05, // protocol level 5
            0xCE, // flags: clean_start + will + QoS1 + password + username
            0x00, 0x3C, // keep alive = 60
            0x00, // connect properties length = 0
            0x00, 0x04, 0x54, 0x45, 0x53, 0x54, // client_id "TEST"
            0x00, // will properties length = 0
            0x00, 0x03, 0x74, 0x2F, 0x77, // will topic "t/w"
            0x00, 0x02, 0x01, 0x02, // will data [0x01, 0x02]
            0x00, 0x04, 0x75, 0x73, 0x65, 0x72, // username "user"
            0x00, 0x04, 0x70, 0x61, 0x73, 0x73, // password "pass"
        ];

        let connect = Connect {
            protocol: Protocol::MQTT5,
            properties: Properties::Slice(&[]),
            keep_alive: 60,
            client_id: "TEST",
            username: Some("user"),
            password: Some(b"pass"),
            last_will: Some(LastWill {
                topic: "t/w",
                data: &[0x01, 0x02],
                qos: QoS::AtLeastOnce,
                retained: false,
                properties: Properties::Slice(&[]),
            }),
            clean_start: true,
        };

        let mut buf = [0u8; 64];
        let mut encoder = MqttEncoder::new(&mut buf);
        connect.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv3")]
    #[test]
    fn encode_connect_v311_username_password() {
        let expected_bytes = &[
            0x10, 0x1C, // CONNECT, remaining length = 28
            0x00, 0x04, 0x4D, 0x51, 0x54, 0x54, // "MQTT"
            0x04, // protocol level 4 (v3.1.1)
            0xC2, // flags: clean_start + username + password
            0x00, 0x3C, // keep alive = 60
            0x00, 0x04, 0x54, 0x45, 0x53, 0x54, // client_id "TEST"
            0x00, 0x04, 0x75, 0x73, 0x65, 0x72, // username "user"
            0x00, 0x04, 0x70, 0x61, 0x73, 0x73, // password "pass"
        ];

        let connect = Connect {
            protocol: Protocol::MQTT311,
            keep_alive: 60,
            client_id: "TEST",
            username: Some("user"),
            password: Some(b"pass"),
            last_will: None,
            clean_start: true,
        };

        let mut buf = [0u8; 64];
        let mut encoder = MqttEncoder::new(&mut buf);
        connect.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv3")]
    #[test]
    fn encode_connect_v311_last_will() {
        let expected_bytes = &[
            0x10, 0x19, // CONNECT, remaining length = 25
            0x00, 0x04, 0x4D, 0x51, 0x54, 0x54, // "MQTT"
            0x04, // protocol level 4
            0x0E, // flags: clean_start + will + QoS1
            0x00, 0x3C, // keep alive = 60
            0x00, 0x04, 0x54, 0x45, 0x53, 0x54, // client_id "TEST"
            0x00, 0x03, 0x74, 0x2F, 0x77, // will topic "t/w"
            0x00, 0x02, 0x01, 0x02, // will data [0x01, 0x02]
        ];

        let connect = Connect {
            protocol: Protocol::MQTT311,
            keep_alive: 60,
            client_id: "TEST",
            username: None,
            password: None,
            last_will: Some(LastWill {
                topic: "t/w",
                data: &[0x01, 0x02],
                qos: QoS::AtLeastOnce,
                retained: false,
            }),
            clean_start: true,
        };

        let mut buf = [0u8; 64];
        let mut encoder = MqttEncoder::new(&mut buf);
        connect.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[cfg(feature = "mqttv3")]
    #[test]
    fn encode_connect_v311_all_fields() {
        let expected_bytes = &[
            0x10, 0x25, // CONNECT, remaining length = 37
            0x00, 0x04, 0x4D, 0x51, 0x54, 0x54, // "MQTT"
            0x04, // protocol level 4
            0xCE, // flags: clean_start + will + QoS1 + password + username
            0x00, 0x3C, // keep alive = 60
            0x00, 0x04, 0x54, 0x45, 0x53, 0x54, // client_id "TEST"
            0x00, 0x03, 0x74, 0x2F, 0x77, // will topic "t/w"
            0x00, 0x02, 0x01, 0x02, // will data [0x01, 0x02]
            0x00, 0x04, 0x75, 0x73, 0x65, 0x72, // username "user"
            0x00, 0x04, 0x70, 0x61, 0x73, 0x73, // password "pass"
        ];

        let connect = Connect {
            protocol: Protocol::MQTT311,
            keep_alive: 60,
            client_id: "TEST",
            username: Some("user"),
            password: Some(b"pass"),
            last_will: Some(LastWill {
                topic: "t/w",
                data: &[0x01, 0x02],
                qos: QoS::AtLeastOnce,
                retained: false,
            }),
            clean_start: true,
        };

        let mut buf = [0u8; 64];
        let mut encoder = MqttEncoder::new(&mut buf);
        connect.to_buffer(&mut encoder).unwrap();

        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }
}
