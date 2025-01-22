use bon::Builder;
use core::fmt::Debug;

use crate::{
    encoder::MAX_MQTT_HEADER_LEN,
    encoding::{
        encoder::{MqttEncode, MqttEncoder},
        error::Error,
        properties::Properties,
        utils::{Pid, QoS},
        FixedHeader,
    },
    varint_len, StateError,
};
use embedded_io_async::{Error as _, ErrorKind, Read, ReadExactError};

use super::PacketType;

pub trait ToPayload {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error>;
    fn max_size(&self) -> usize;
}

impl<'a> ToPayload for &'a [u8] {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        if buffer.len() < self.len() {
            return Err(Error::BufferSize);
        }
        buffer[..self.len()].copy_from_slice(self);
        Ok(self.len())
    }

    fn max_size(&self) -> usize {
        self.len()
    }
}

impl<const N: usize> ToPayload for [u8; N] {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        (&self[..]).serialize(buffer)
    }

    fn max_size(&self) -> usize {
        self.len()
    }
}

impl<const N: usize> ToPayload for &[u8; N] {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        (&self[..]).serialize(buffer)
    }

    fn max_size(&self) -> usize {
        self.len()
    }
}

pub struct DeferredPayload<F: Fn(&mut [u8]) -> Result<usize, Error>> {
    func: F,
    max_len: usize,
}

impl<F: Fn(&mut [u8]) -> Result<usize, Error>> DeferredPayload<F> {
    pub fn new(func: F, max_len: usize) -> Self {
        Self { func, max_len }
    }
}

impl<F: Fn(&mut [u8]) -> Result<usize, Error>> ToPayload for DeferredPayload<F> {
    fn serialize(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        if buffer.len() < self.max_len {
            return Err(Error::BufferSize);
        }
        (self.func)(buffer)
    }

    fn max_size(&self) -> usize {
        self.max_len
    }
}

/// Publish packet ([MQTT 3.3]).
///
/// [MQTT 3.3]: https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc3901100
#[derive(Debug, Clone, PartialEq, Builder)]
pub struct Publish<'a, P: ToPayload> {
    #[builder(default = false)]
    pub(crate) dup: bool,

    #[builder(default = QoS::AtLeastOnce)]
    pub(crate) qos: QoS,

    #[builder(default = false)]
    pub(crate) retain: bool,

    #[builder(skip)]
    pub(crate) pid: Option<Pid>,
    pub(crate) topic_name: &'a str,
    pub(crate) payload: P,

    #[cfg(feature = "mqttv5")]
    #[builder(default = Properties::Slice(&[]))]
    pub(crate) properties: Properties<'a>,
}

impl<'a, P: ToPayload> FixedHeader for Publish<'a, P> {
    const PACKET_TYPE: PacketType = PacketType::Publish;

    fn flags(&self) -> u8 {
        let mut flags = u8::from(self.qos) << 1;
        if self.dup {
            flags |= 0b1000;
        };
        if self.retain {
            flags |= 0b0001;
        };

        flags
    }
}

impl<'a, P: ToPayload> MqttEncode for Publish<'a, P> {
    fn to_buffer(&self, encoder: &mut MqttEncoder) -> Result<(), Error> {
        // Topic
        encoder.write_str(self.topic_name)?;

        match self.qos {
            QoS::AtMostOnce => (),
            QoS::AtLeastOnce => {
                encoder.write_u16(self.pid.ok_or(Error::PidMissing)?.get())?;
            }
            #[cfg(feature = "qos2")]
            QoS::ExactlyOnce => {
                encoder.write_u16(self.pid.ok_or(Error::PidMissing)?.get())?;
            }
        }

        // Properties
        #[cfg(feature = "mqttv5")]
        encoder.write_properties(&self.properties)?;

        // Payload
        encoder.write_payload(&self.payload)?;

        encoder.finalize_fixed_header(self)?;

        Ok(())
    }

    fn set_pid(&mut self, pid: Pid) {
        self.pid.replace(pid);
    }

    fn get_qos(&self) -> Option<QoS> {
        Some(self.qos)
    }

    fn max_packet_size(&self) -> usize {
        let mut length = 2 + self.topic_name.len()
            + match self.qos {
                QoS::AtMostOnce => 0,
                _ => 2,
            } // pid
            + MAX_MQTT_HEADER_LEN;

        #[cfg(feature = "mqttv5")]
        {
            length += varint_len(self.properties.size());
        }

        length += self.payload.max_size();

        length
    }
}

pub(crate) struct PartialPublish<'a, S: Read> {
    buf: &'a [u8],
    packet_len: usize,
    reader: &'a mut S,
}

impl<'a, S: Read> Debug for PartialPublish<'a, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PartialPublish")
            .field("buf", &self.buf)
            .field("packet_len", &self.packet_len)
            .finish()
    }
}

impl<'a, S: Read> PartialPublish<'a, S> {
    pub fn new(buf: &'a [u8], packet_len: usize, reader: &'a mut S) -> Self {
        Self {
            buf,
            packet_len,
            reader,
        }
    }

    pub fn len(&self) -> usize {
        self.packet_len
    }

    pub async fn copy_all(&mut self, buf: &mut [u8]) -> Result<(), StateError> {
        buf[..self.buf.len()].copy_from_slice(self.buf);

        if self.buf.len() < self.len() {
            self.reader
                .read_exact(&mut buf[self.buf.len()..self.len()])
                .await
                .map_err(|e| match e {
                    ReadExactError::UnexpectedEof => StateError::Io(ErrorKind::BrokenPipe),
                    ReadExactError::Other(i) => StateError::Io(i.kind()),
                })?;
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn as_publish(&self) -> Publish<'_, &[u8]> {
        assert!(self.buf.len() >= self.len());

        let mut decoder = crate::decoder::MqttDecoder::try_new(self.buf).unwrap();
        let header = decoder.fixed_header();

        assert_eq!(header.typ, PacketType::Publish);

        decoder.check_remaining(header.remaining_len).unwrap();

        let topic_name = decoder.read_str().unwrap();
        let pid = match header.qos {
            QoS::AtMostOnce => None,
            QoS::AtLeastOnce => Pid::try_from(decoder.read_u16().unwrap()).ok(),
            #[cfg(feature = "qos2")]
            QoS::ExactlyOnce => Pid::try_from(decoder.read_u16().unwrap()).ok(),
        };

        #[cfg(feature = "mqttv5")]
        let properties = decoder.read_properties().unwrap();
        let payload = decoder.read_payload().unwrap();

        Publish {
            dup: header.dup,
            qos: header.qos,
            retain: header.retain,
            pid,
            topic_name,
            payload,
            #[cfg(feature = "mqttv5")]
            properties,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::received_packet::ReceivedPacket;
    use crate::Property;

    #[derive(Debug)]
    pub struct MockRead;

    impl embedded_io_async::Read for MockRead {
        async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, Self::Error> {
            todo!()
        }
    }

    impl embedded_io_async::ErrorType for MockRead {
        type Error = core::convert::Infallible;
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    fn encode_publish_bytes_v5() {
        let expected_bytes = &[
            50, 57, 0, 21, 109, 121, 47, 112, 101, 114, 115, 111, 110, 97, 108, 105, 122, 101, 100,
            47, 116, 111, 112, 105, 99, 0, 1, 0, 84, 104, 105, 115, 32, 105, 115, 32, 109, 121, 32,
            97, 119, 101, 115, 111, 109, 101, 32, 98, 121, 116, 101, 32, 112, 97, 121, 108, 111,
            97, 100,
        ];

        let publish = Publish {
            dup: false,
            qos: QoS::AtLeastOnce,
            retain: false,
            pid: Some(Pid::new()),
            topic_name: "my/personalized/topic",
            payload: b"This is my awesome byte payload",
            properties: Properties::Slice(&[]),
        };

        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        publish.to_buffer(&mut encoder).unwrap();

        assert_eq!(publish.max_packet_size(), 62);
        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    fn encode_publish_closure_v5() {
        let expected_bytes = &[
            50, 57, 0, 21, 109, 121, 47, 112, 101, 114, 115, 111, 110, 97, 108, 105, 122, 101, 100,
            47, 116, 111, 112, 105, 99, 0, 1, 0, 84, 104, 105, 115, 32, 105, 115, 32, 109, 121, 32,
            97, 119, 101, 115, 111, 109, 101, 32, 98, 121, 116, 101, 32, 112, 97, 121, 108, 111,
            97, 100,
        ];

        let payload_bytes = b"This is my awesome byte payload";

        let payload_closure = |buf: &mut [u8]| {
            buf[..payload_bytes.len()].copy_from_slice(payload_bytes);
            Ok(payload_bytes.len())
        };

        let publish = Publish {
            dup: false,
            qos: QoS::AtLeastOnce,
            retain: false,
            pid: Some(Pid::new()),
            topic_name: "my/personalized/topic",
            payload: DeferredPayload::new(payload_closure, payload_bytes.len()),
            properties: Properties::Slice(&[]),
        };

        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        publish.to_buffer(&mut encoder).unwrap();

        assert_eq!(publish.max_packet_size(), 62);
        assert_eq!(encoder.packet_bytes(), expected_bytes);
    }

    #[test]
    #[cfg(feature = "mqttv5")]
    /// Test case: Simple publish with payload and properties
    fn test_publish_v5_encode_decode_received_packet() {
        let props = &[
            Property::PayloadFormatIndicator(0),
            Property::MessageExpiryInterval(1000),
            Property::ContentType("text/plain".into()),
            Property::ResponseTopic("response/topic".into()),
            Property::CorrelationData(b"correlation_data"),
            Property::UserProperty("key1".into(), "value1".into()),
            Property::UserProperty("key2".into(), "value2".into()),
        ];
        let publish_v5 = Publish {
            dup: false,
            qos: QoS::AtLeastOnce,
            retain: false,
            pid: Some(Pid::new()),
            topic_name: "test/topic",
            payload: b"Hello, world!",
            properties: Properties::Slice(props),
        };

        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);
        publish_v5.to_buffer(&mut encoder).unwrap();

        let mut reader = MockRead;
        let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

        if let ReceivedPacket::PartialPublish {
            qos_pid,
            topic_name,
            publish,
        } = packet
        {
            let publish = publish.as_publish();
            assert_eq!(publish.qos, QoS::AtLeastOnce);
            assert_eq!(publish.dup, false);
            assert_eq!(publish.retain, false);
            assert_eq!(qos_pid, crate::QosPid::AtLeastOnce(Pid::new()));
            assert_eq!(topic_name, "test/topic");
            assert_eq!(publish.payload, b"Hello, world!");

            let mut properties = publish.properties.iter();
            assert_eq!(
                properties.next().unwrap().unwrap().property_id(),
                Property::PayloadFormatIndicator(0).property_id()
            );
            assert_eq!(
                properties.next().unwrap().unwrap().property_id(),
                Property::MessageExpiryInterval(1000).property_id()
            );
            assert_eq!(
                properties.next().unwrap().unwrap().property_id(),
                Property::ContentType("text/plain".into()).property_id()
            );
            assert_eq!(
                properties.next().unwrap().unwrap().property_id(),
                Property::ResponseTopic("response/topic".into()).property_id()
            );
            assert_eq!(
                properties.next().unwrap().unwrap().property_id(),
                Property::CorrelationData(b"correlation_data").property_id()
            );
            assert_eq!(
                properties.next().unwrap().unwrap().property_id(),
                Property::UserProperty("key1".into(), "value1".into()).property_id()
            );
            assert_eq!(
                properties.next().unwrap().unwrap().property_id(),
                Property::UserProperty("key2".into(), "value2".into()).property_id()
            );
            assert!(properties.next().is_none());
        } else {
            panic!("Expected Publish packet");
        }
    }

    // #[test]
    // #[cfg(feature = "mqttv5")]
    // fn test_connack_v5_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let packet = crate::ConnAck {
    //         session_present: true,
    //         reason_code: crate::encoding::reason_code::ConnAckReasonCode::Success,
    //         properties: Properties::Slice(&[Property::SessionExpiryInterval(1000)]),
    //     };

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::ConnAck {
    //         session_present,
    //         reason_code,
    //         properties,
    //     } = packet
    //     {
    //         assert_eq!(session_present, true);
    //         assert_eq!(
    //             reason_code,
    //             crate::encoding::reason_code::ConnAckReasonCode::Success
    //         );

    //         let mut properties = properties.iter();
    //         assert_eq!(
    //             properties.next().unwrap().unwrap().property_id(),
    //             Property::SessionExpiryInterval(1000).property_id()
    //         );

    //         assert!(properties.next().is_none());
    //     } else {
    //         panic!("Expected ConnAck packet");
    //     }
    // }

    #[test]
    #[cfg(all(feature = "mqttv5", feature = "qos2"))]
    fn test_puback_v5_encode_decode_received_packet() {
        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);

        let mut packet = Packet::new(PacketType::PubAck);
        packet.puback_mut().unwrap().pid = Pid::new();
        packet.puback_mut().unwrap().reason_code = PubAckReasonCode::Success;
        packet.puback_mut().unwrap().properties =
            Properties::Slice(&[Property::ResponseTopic("response/topic".into())]);

        packet.to_buffer(&mut encoder).unwrap();

        let mut reader = MockRead;
        let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

        if let ReceivedPacket::PubAck {
            pid,
            _reason_code,
            _properties,
        } = packet
        {
            assert_eq!(pid, Pid::new());
        } else {
            panic!("Expected PubAck packet");
        }
    }

    // #[test]
    // #[cfg(all(feature = "mqttv5", feature = "qos2"))]
    // fn test_pubrec_v5_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = Packet::new(PacketType::PubRec);
    //     packet.pubrec_mut().unwrap().pid = Pid::new();
    //     packet.pubrec_mut().unwrap().reason_code = PubRecReasonCode::Success;
    //     packet.pubrec_mut().unwrap().properties =
    //         Properties::Slice(&[Property::ResponseTopic("response/topic".into())]);

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::PubRec {
    //         pid,
    //         reason_code,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(pid, Pid::new());
    //         assert_eq!(reason_code, PubRecReasonCode::Success);
    //     } else {
    //         panic!("Expected PubRec packet");
    //     }
    // }

    // #[test]
    // #[cfg(all(feature = "mqttv5", feature = "qos2"))]
    // fn test_pubrel_v5_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = Packet::new(PacketType::PubRel);
    //     packet.pubrel_mut().unwrap().pid = Pid::new();
    //     packet.pubrel_mut().unwrap().reason_code = PubRelReasonCode::Success;
    //     packet.pubrel_mut().unwrap().properties =
    //         Properties::Slice(&[Property::ResponseTopic("response/topic".into())]);

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::PubRel {
    //         pid,
    //         reason_code,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(pid, Pid::new());
    //         assert_eq!(reason_code, PubRelReasonCode::Success);
    //     } else {
    //         panic!("Expected PubRel packet");
    //     }
    // }

    // #[test]
    // #[cfg(all(feature = "mqttv5", feature = "qos2"))]
    // fn test_pubcomp_v5_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = Packet::new(PacketType::PubComp);
    //     packet.pubcomp_mut().unwrap().pid = Pid::new();
    //     packet.pubcomp_mut().unwrap().reason_code = PubCompReasonCode::Success;
    //     packet.pubcomp_mut().unwrap().properties =
    //         Properties::Slice(&[Property::ResponseTopic("response/topic".into())]);

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::PubComp {
    //         pid,
    //         reason_code,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(pid, Pid::new());
    //         assert_eq!(reason_code, PubCompReasonCode::Success);
    //     } else {
    //         panic!("Expected PubComp packet");
    //     }
    // }

    #[test]
    fn test_pingresp_encode_decode_received_packet() {
        let mut buf = [0u8; 128];
        let mut encoder = MqttEncoder::new(&mut buf);

        let packet = crate::PingResp;

        packet.to_buffer(&mut encoder).unwrap();

        let mut reader = MockRead;
        let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

        assert!(matches!(packet, ReceivedPacket::PingResp));
    }

    // #[test]
    // #[cfg(feature = "mqttv3")]
    // fn test_disconnect_v311_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = crate::Disconnect {
    //         reason_code: crate::encoding::reason_code::DisconnectReasonCode::Normal,
    //     };
    //     packet.disconnect_mut().unwrap().reason_code = 0;

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::Disconnect {
    //         reason_code,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(reason_code, 0);
    //     } else {
    //         panic!("Expected Disconnect packet");
    //     }
    // }

    // #[test]
    // #[cfg(feature = "mqttv5")]
    // fn test_disconnect_v5_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = Packet::new(PacketType::Disconnect);
    //     packet.disconnect_mut().unwrap().reason_code = 0;
    //     packet.disconnect_mut().unwrap().properties =
    //         Properties::Slice(&[Property::ReasonString("Normal Disconnection".into())]);

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::Disconnect {
    //         reason_code,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(reason_code, 0);
    //     } else {
    //         panic!("Expected Disconnect packet");
    //     }
    // }

    // #[test]
    // #[cfg(feature = "mqttv3")]
    // fn test_suback_v311_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = Packet::new(PacketType::SubAck);
    //     packet.suback_mut().unwrap().pid = Pid::new();
    //     packet.suback_mut().unwrap().codes = &[0, 1, 2].try_into().unwrap();

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::SubAck {
    //         pid,
    //         codes,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(pid, Pid::new());
    //         assert_eq!(codes, &[0, 1, 2]);
    //     } else {
    //         panic!("Expected SubAck packet");
    //     }
    // }

    // #[test]
    // #[cfg(feature = "mqttv5")]
    // fn test_suback_v5_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = Packet::new(PacketType::SubAck);
    //     packet.suback_mut().unwrap().pid = Pid::new();
    //     packet.suback_mut().unwrap().codes = &[0, 1, 2].try_into().unwrap();
    //     packet.suback_mut().unwrap().properties =
    //         Properties::Slice(&[Property::ReasonString("Success".into())]);

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::SubAck {
    //         pid,
    //         codes,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(pid, Pid::new());
    //         assert_eq!(codes, &[0, 1, 2]);
    //     } else {
    //         panic!("Expected SubAck packet");
    //     }
    // }

    // #[test]
    // #[cfg(feature = "mqttv3")]
    // fn test_unsuback_v311_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = Packet::new(PacketType::UnsubAck);
    //     packet.unsuback_mut().unwrap().pid = Pid::new();

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::UnsubAck {
    //         pid,
    //         codes,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(pid, Pid::new());
    //         assert_eq!(codes, &[]);
    //     } else {
    //         panic!("Expected UnsubAck packet");
    //     }
    // }

    // #[test]
    // #[cfg(feature = "mqttv5")]
    // fn test_unsuback_v5_encode_decode_received_packet() {
    //     let mut buf = [0u8; 128];
    //     let mut encoder = MqttEncoder::new(&mut buf);

    //     let mut packet = Packet::new(PacketType::UnsubAck);
    //     packet.unsuback_mut().unwrap().pid = Pid::new();
    //     packet.unsuback_mut().unwrap().properties =
    //         Properties::Slice(&[Property::ReasonString("Success".into())]);

    //     packet.to_buffer(&mut encoder).unwrap();

    //     let mut reader = MockRead;
    //     let packet = ReceivedPacket::from_buffer(encoder.packet_bytes(), &mut reader).unwrap();

    //     if let ReceivedPacket::UnsubAck {
    //         pid,
    //         codes,
    //         _properties,
    //     } = packet
    //     {
    //         assert_eq!(pid, Pid::new());
    //         assert_eq!(codes, &[]);
    //     } else {
    //         panic!("Expected UnsubAck packet");
    //     }
    // }
}
