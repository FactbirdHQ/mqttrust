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

#[derive(Debug)]
pub(crate) enum ReceivedPacket<'a, R: Read> {
    PingResp,
    ConnAck {
        /// Indicates true if session state is being maintained by the broker.
        session_present: bool,

        /// A status code indicating the success status of the connection.
        reason_code: ConnAckReasonCode,

        /// A list of properties associated with the connection.
        #[cfg(feature = "mqttv5")]
        properties: Properties<'a>,
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
        /// The identifier that the acknowledge is associated with.
        pid: Pid,

        /// The optional properties associated with the acknowledgement.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,

        /// The response status code of the subscription request.
        // codes: &'a [SubAckReturnCode],
        codes: &'a [u8],
    },
    UnsubAck {
        /// The ID of the packet being acknowledged.
        pid: Pid,

        /// The properties associated with this packet.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,

        /// The response status code of the subscription request.
        // codes: &'a [UnsubAckReturnCode],
        codes: &'a [u8],
    },
    Disconnect {
        /// A status code indicating the success status of the connection.
        reason_code: u8,

        /// A list of properties associated with the connection.
        #[cfg(feature = "mqttv5")]
        _properties: Properties<'a>,
    },
}

impl<'a, R: Read> ReceivedPacket<'a, R> {
    pub(crate) fn from_buffer(buf: &'a [u8], reader: &'a mut R) -> Result<Self, Error> {
        let mut decoder = MqttDecoder::try_new(buf)?;
        let header = decoder.fixed_header();

        debug!("Read header {:?}", header);

        let packet = match header.typ {
            PacketType::Unsubscribe => {
                Pid::try_from(decoder.read_u16()?)?;
                #[cfg(feature = "mqttv5")]
                decoder.read_properties()?;
                let topic = decoder.read_str()?;

                debug!("WTF {:?}", topic);

                return Err(Error::InvalidHeader);
            }
            PacketType::Disconnect => {
                if header.remaining_len < 2 {
                    Self::Disconnect {
                        reason_code: 0,
                        #[cfg(feature = "mqttv5")]
                        _properties: Properties::DataBlock(&[]),
                    }
                } else {
                    Self::Disconnect {
                        reason_code: decoder.read_u8()?,
                        #[cfg(feature = "mqttv5")]
                        _properties: decoder.read_properties()?,
                    }
                }
            }
            PacketType::PingResp => Self::PingResp,
            PacketType::ConnAck => {
                let conn_ack_flags = decoder.read_u8()?;

                if conn_ack_flags & 0b11111110 != 0 {
                    return Err(Error::MalformedPacket);
                }

                Self::ConnAck {
                    session_present: (conn_ack_flags & 0b1 == 1),
                    reason_code: ConnAckReasonCode::from(decoder.read_u8()?),
                    #[cfg(feature = "mqttv5")]
                    properties: decoder.read_properties()?,
                }
            }
            PacketType::Publish => {
                let _topic_name = decoder.read_str()?;

                let qos_pid = match header.qos {
                    QoS::AtMostOnce => QosPid::AtMostOnce,
                    QoS::AtLeastOnce => QosPid::AtLeastOnce(Pid::try_from(decoder.read_u16()?)?),
                    #[cfg(feature = "qos2")]
                    QoS::ExactlyOnce => QosPid::ExactlyOnce(Pid::try_from(decoder.read_u16()?)?),
                };

                debug!(
                    "{:?}: {} bytes [{:?}]",
                    qos_pid,
                    decoder.packet_len(),
                    _topic_name
                );

                let buf_len = core::cmp::min(decoder.packet_len(), buf.len());

                Self::Publish {
                    qos_pid,
                    publish: PartialPublish::new(&buf[..buf_len], decoder.packet_len(), reader),
                }
            }
            PacketType::PubAck => {
                if header.remaining_len == 2 || header.remaining_len == 3 {
                    Self::PubAck {
                        pid: Pid::try_from(decoder.read_u16()?)?,
                        #[cfg(feature = "mqttv5")]
                        _reason_code: PubAckReasonCode::Success,
                        #[cfg(feature = "mqttv5")]
                        _properties: Properties::DataBlock(&[]),
                    }
                } else {
                    Self::PubAck {
                        pid: Pid::try_from(decoder.read_u16()?)?,
                        #[cfg(feature = "mqttv5")]
                        _reason_code: PubAckReasonCode::from(decoder.read_u8()?),
                        #[cfg(feature = "mqttv5")]
                        _properties: decoder.read_properties()?,
                    }
                }
            }
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
                codes: decoder.read_payload()?,
            },
            PacketType::UnsubAck => Self::UnsubAck {
                pid: Pid::try_from(decoder.read_u16()?)?,
                #[cfg(feature = "mqttv5")]
                _properties: decoder.read_properties()?,
                codes: decoder.read_payload()?,
            },
            _ => return Err(Error::InvalidHeader),
        };

        Ok(packet)
    }
}
