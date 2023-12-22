use crate::encoding::{
    decoder::MqttDecoder,
    error::Error,
    packets::PacketType,
    utils::{Pid, QosPid},
    PartialPublish, QoS,
};

#[cfg(feature = "mqttv5")]
use crate::encoding::reason_code::{ConnAckReasonCode, PubAckReasonCode};

#[cfg(all(feature = "mqttv5", feature = "qos2"))]
use crate::encoding::reason_code::{PubCompReasonCode, PubRecReasonCode, PubRelReasonCode};

#[cfg(feature = "mqttv5")]
use crate::Properties;

#[cfg(feature = "mqttv3")]
use crate::encoding::reason_code::{ConnAckReasonCode, SubAckReturnCode};

use embedded_io_async::Read;

pub(crate) enum ReceivedPacket<'a, R: Read, const N: usize> {
    PingResp,
    ConnAck {
        /// Indicates true if session state is being maintained by the broker.
        session_present: bool,

        /// A status code indicating the success status of the connection.
        reason_code: ConnAckReasonCode,

        /// A list of properties associated with the connection.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,
    },
    Publish {
        qos_pid: QosPid,
        publish: PartialPublish<'a, R>,
    },
    PubAck {
        /// The ID of the packet being acknowledged.
        pid: Pid,

        /// The reason code associated with the publication.
        #[cfg(feature = "mqttv5")]
        _reason_code: PubAckReasonCode,

        /// The properties associated with the publication.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,
    },
    #[cfg(feature = "qos2")]
    PubRec {
        /// The ID of the packet that publication reception occurred on.
        pid: Pid,

        /// The reason code associated with the publication.
        #[cfg(feature = "mqttv5")]
        reason_code: PubRecReasonCode,

        /// The properties associated with the publication.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,
    },
    #[cfg(feature = "qos2")]
    PubRel {
        /// The ID of the publication that this packet is associated with.
        pid: Pid,

        /// The properties and success status of associated with the publication.
        #[cfg(feature = "mqttv5")]
        reason_code: PubRelReasonCode,

        /// The properties associated with the publication.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,
    },
    #[cfg(feature = "qos2")]
    PubComp {
        /// Packet identifier of the publication that this packet is associated with.
        pid: Pid,

        /// The reason code associated with this packet.
        #[cfg(feature = "mqttv5")]
        reason_code: PubCompReasonCode,

        /// The properties associated with this packet.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,
    },
    SubAck {
        /// The identifier that the acknowledge is assocaited with.
        pid: Pid,

        /// The optional properties associated with the acknowledgement.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,

        /// The response status code of the subscription request.
        #[cfg(feature = "mqttv3")]
        codes: &'a [SubAckReturnCode],
    },
    UnsubAck {
        /// The ID of the packet being acknowledged.
        pid: Pid,

        /// The properties associated with this packet.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,
    },
}

impl<'a, R: Read, const N: usize> ReceivedPacket<'a, R, N> {
    pub(crate) fn from_buffer(buf: &'a [u8], reader: &'a mut R) -> Result<Self, Error> {
        let mut decoder = MqttDecoder::new(buf);
        let header = decoder.read_fixed_header()?;

        debug!("Read header {:?}", header.typ);

        let packet_len = decoder.packet_len().ok_or(Error::InvalidLength)?;

        let packet = match header.typ {
            PacketType::PingResp => Self::PingResp,
            PacketType::ConnAck => Self::ConnAck {
                session_present: (decoder.read_u8()? & 0b1 == 1),
                reason_code: ConnAckReasonCode::from(decoder.read_u8()?),
                #[cfg(feature = "mqttv5")]
                _properties: decoder.read_properties()?,
            },
            PacketType::Publish => {
                let _topic_name = decoder.read_str()?;

                let qos_pid = match header.qos {
                    QoS::AtMostOnce => QosPid::AtMostOnce,
                    QoS::AtLeastOnce => QosPid::AtLeastOnce(Pid::try_from(decoder.read_u16()?)?),
                    #[cfg(feature = "qos2")]
                    QoS::ExactlyOnce => QosPid::ExactlyOnce(Pid::try_from(decoder.read_u16()?)?),
                };

                let buf_len = core::cmp::min(packet_len, buf.len());

                Self::Publish {
                    qos_pid,
                    publish: PartialPublish::new(&buf[..buf_len], packet_len, reader),
                }
            }
            PacketType::PubAck => Self::PubAck {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                _reason_code: PubAckReasonCode::from(decoder.read_u8().unwrap_or(0x00)),
                #[cfg(feature = "mqttv5")]
                _properties: decoder.read_properties()?,
            },
            #[cfg(feature = "qos2")]
            PacketType::PubRec => Self::PubRec {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubRecReasonCode::from(decoder.read_u8().unwrap_or(0x00)),
                #[cfg(feature = "mqttv5")]
                _properties: decoder.read_properties()?,
            },
            #[cfg(feature = "qos2")]
            PacketType::PubRel => Self::PubRel {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubRelReasonCode::from(decoder.read_u8().unwrap_or(0x00)),
                #[cfg(feature = "mqttv5")]
                _properties: decoder.read_properties()?,
            },
            #[cfg(feature = "qos2")]
            PacketType::PubComp => Self::PubComp {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                reason_code: PubCompReasonCode::from(decoder.read_u8().unwrap_or(0x00)),
                #[cfg(feature = "mqttv5")]
                _properties: decoder.read_properties()?,
            },
            PacketType::SubAck => Self::SubAck {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                _properties: decoder.read_properties()?,
                #[cfg(feature = "mqttv3")]
                codes: &[],
            },
            PacketType::UnsubAck => Self::UnsubAck {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                _properties: decoder.read_properties()?,
            },
            _ => return Err(Error::InvalidHeader),
        };

        Ok(packet)
    }
}
