use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::encoding::{
    encoder::{MqttEncode, MqttEncoder},
    error::Error,
    utils::QoS,
    FixedHeader,
};

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
#[derive(Debug, Clone, PartialEq)]
pub struct LastWill<'a> {
    pub topic: &'a str,
    pub data: &'a [u8],
    pub qos: QoS,
    pub retained: bool,
    #[cfg(feature = "mqttv5")]
    pub properties: Properties<'a>,
}

/// Connect packet ([MQTT 3.1]).
///
/// [MQTT 3.1]: https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc3901033
#[derive(Debug, Clone, PartialEq)]
pub struct Connect<'a> {
    pub protocol: Protocol,

    /// Any properties associated with the CONNECT request.
    #[cfg(feature = "mqttv5")]
    pub properties: Properties<'a>,

    /// Specifies the keep-alive interval of the connection in seconds.
    pub keep_alive: u16,

    /// The ID of the client that is connecting. May be an empty string to automatically allocate
    /// an ID from the broker.
    pub client_id: &'a str,

    /// An optional authentication message used by the server.
    pub username: Option<&'a str>,
    pub password: Option<&'a [u8]>,

    /// An optional will message to be transmitted whenever the connection is lost.
    pub(crate) last_will: Option<LastWill<'a>>,

    /// Specified true there is no session state being taken in to the MQTT connection.
    pub clean_start: bool,
}

impl<'a> FixedHeader for Connect<'a> {
    const PACKET_TYPE: PacketType = PacketType::Connect;

    fn remaining_len(&self) -> usize {
        let mut length: usize = 6 + 1 + 1; // NOTE: protocol_name(6) + protocol_level(1) + flags(1);
        length += 2 + self.client_id.len();
        length += 2; // keep alive
        if let Some(username) = self.username {
            length += username.len();
            length += 2;
        };
        if let Some(password) = self.password {
            length += password.len();
            length += 2;
        };

        // TODO: Properties

        if let Some(last_will) = &self.last_will {
            // TODO: Properties
            length += last_will.topic.len();
            length += last_will.data.len();
            length += 4;
        };

        length
    }
}

impl MqttEncode for Connect<'_> {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        encoder.write_fixed_header(self)?;

        encoder.write_str(self.protocol.name())?;
        encoder.write_u8(self.protocol.into())?;

        // TOOD: Change `connect_flags` to bitflags
        let mut connect_flags: u8 = 0b00000000;
        if self.clean_start {
            connect_flags |= 0b10;
        };
        if self.username.is_some() {
            connect_flags |= 0b10000000;
        };
        if self.password.is_some() {
            connect_flags |= 0b01000000;
        };

        if let Some(last_will) = &self.last_will {
            connect_flags |= 0b00000100;
            connect_flags |= u8::from(last_will.qos) << 3;
            if last_will.retained {
                connect_flags |= 0b00100000;
            };
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

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_connect_v5() {
        let connect = Connect {
            protocol: Protocol::MQTT5,
            #[cfg(feature = "mqttv5")]
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

        let serialized = encoder.bytes();

        assert_eq!(
            &serialized,
            &[16, 17, 0, 4, 77, 81, 84, 84, 5, 2, 0, 60, 0, 0, 4, 84, 69, 83, 84]
        );

        // assert_eq!(
        //     Connect::from_buffer(&buf[..], &mut offset).unwrap(),
        //     connect
        // );
    }
}
