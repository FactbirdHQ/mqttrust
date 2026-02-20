use crate::{
    decoder::MqttDecode,
    encoding::{
        decoder::MqttDecoder,
        error::Error,
        packets::PacketType,
        utils::{Pid, QosPid},
        PartialPublish, QoS,
    },
};

use embedded_io_async::Read;

use super::{ConnAck, Disconnect, PubAck, SubAck, UnsubAck};

#[cfg(feature = "qos2")]
use super::{PubComp, PubRec, PubRel};

#[derive(Debug)]
pub(crate) enum ReceivedPacket<'a, R: Read> {
    PingResp,
    ConnAck(ConnAck<'a>),
    PartialPublish {
        qos_pid: QosPid,
        topic_name: &'a str,
        publish: PartialPublish<'a, R>,
    },
    PubAck(PubAck<'a>),
    #[cfg(feature = "qos2")]
    PubRec(PubRec<'a>),
    #[cfg(feature = "qos2")]
    PubRel(PubRel<'a>),
    #[cfg(feature = "qos2")]
    PubComp(PubComp<'a>),
    SubAck(SubAck<'a>),
    UnsubAck(UnsubAck<'a>),
    Disconnect(Disconnect<'a>),
}

impl<'a, R: Read> ReceivedPacket<'a, R> {
    pub(crate) fn from_buffer(buf: &'a [u8], reader: &'a mut R) -> Result<Self, Error> {
        let mut decoder = MqttDecoder::try_new(buf)?;
        let header = decoder.fixed_header();

        // debug!("Read header {:?}", header);

        let packet = match header.typ {
            PacketType::Disconnect => Self::Disconnect(Disconnect::from_decoder(&mut decoder)?),
            PacketType::PingResp => Self::PingResp,
            PacketType::ConnAck => Self::ConnAck(ConnAck::from_decoder(&mut decoder)?),
            PacketType::Publish => {
                let topic_name = decoder.read_str()?;

                let qos_pid = match header.qos {
                    QoS::AtMostOnce => QosPid::AtMostOnce,
                    QoS::AtLeastOnce => QosPid::AtLeastOnce(Pid::try_from(decoder.read_u16()?)?),
                    #[cfg(feature = "qos2")]
                    QoS::ExactlyOnce => QosPid::ExactlyOnce(Pid::try_from(decoder.read_u16()?)?),
                };

                let buf_len = core::cmp::min(decoder.packet_len(), buf.len());

                Self::PartialPublish {
                    qos_pid,
                    topic_name,
                    publish: PartialPublish::new(&buf[..buf_len], decoder.packet_len(), reader),
                }
            }
            PacketType::PubAck => Self::PubAck(PubAck::from_decoder(&mut decoder)?),
            #[cfg(feature = "qos2")]
            PacketType::PubRec => Self::PubRec(PubRec::from_decoder(&mut decoder)?),
            #[cfg(feature = "qos2")]
            PacketType::PubRel => Self::PubRel(PubRel::from_decoder(&mut decoder)?),
            #[cfg(feature = "qos2")]
            PacketType::PubComp => Self::PubComp(PubComp::from_decoder(&mut decoder)?),
            PacketType::SubAck => Self::SubAck(SubAck::from_decoder(&mut decoder)?),
            PacketType::UnsubAck => Self::UnsubAck(UnsubAck::from_decoder(&mut decoder)?),
            _ => return Err(Error::InvalidHeader),
        };

        Ok(packet)
    }
}
