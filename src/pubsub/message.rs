use core::ops::{Deref, DerefMut, Range};

use bitmaps::{Bits, BitsImpl};

use crate::{
    decoder::{FixedHeader, MqttDecoder},
    Pid, QoS, QosPid,
};

use super::{BufferProvider, FrameGrantR};

pub struct Message<'a, B: BufferProvider, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
{
    grant: FrameGrantR<'a, B, SUBS>,
    info: MessageInfo,
}

pub(crate) struct MessageInfo {
    header: FixedHeader,
    qos_pid: QosPid,
    topic_name: Range<usize>,
    payload: Range<usize>,
    #[cfg(feature = "mqttv5")]
    properties: Range<usize>,
}

impl MessageInfo {
    pub(crate) fn try_new<B: BufferProvider, const SUBS: usize>(
        grant: &FrameGrantR<'_, B, SUBS>,
    ) -> Option<Self>
    where
        BitsImpl<{ SUBS }>: Bits,
    {
        let mut decoder = MqttDecoder::try_new(grant.deref()).ok()?;
        // FIXME: would returning result here be more intuitive?

        let header = decoder.fixed_header();
        decoder.check_remaining(header.remaining_len).ok()?;

        let topic_name = {
            let topic_name_start = decoder.offset() + 2;
            let name = decoder.read_str().ok()?;
            warn!("GOT MESSAGE TOPIC: {}", name);
            topic_name_start..decoder.offset()
        };

        let qos_pid = match header.qos {
            QoS::AtMostOnce => QosPid::AtMostOnce,
            QoS::AtLeastOnce => QosPid::AtLeastOnce(Pid::try_from(decoder.read_u16().ok()?).ok()?),
            #[cfg(feature = "qos2")]
            QoS::ExactlyOnce => QosPid::ExactlyOnce(Pid::try_from(decoder.read_u16().ok()?).ok()?),
        };

        #[cfg(feature = "mqttv5")]
        let properties = {
            let properties_start = decoder.offset();
            decoder.read_properties().ok()?;
            properties_start..decoder.offset()
        };

        let payload = {
            let payload_start = decoder.offset();
            decoder.read_payload().ok()?;
            payload_start..decoder.offset()
        };

        Some(Self {
            header,
            qos_pid,
            topic_name,
            #[cfg(feature = "mqttv5")]
            properties,
            payload,
        })
    }

    pub(crate) fn to_message<'a, B: BufferProvider, const SUBS: usize>(
        self,
        mut grant: FrameGrantR<'a, B, SUBS>,
    ) -> Message<'a, B, SUBS>
    where
        BitsImpl<{ SUBS }>: Bits,
    {
        grant.auto_release(true);

        Message { grant, info: self }
    }

    pub(crate) fn topic_name<'a, 'b, B: BufferProvider, const SUBS: usize>(
        &self,
        grant: &'b FrameGrantR<'a, B, SUBS>,
    ) -> &'b str
    where
        BitsImpl<{ SUBS }>: Bits,
    {
        // # Safety: Checked at instantiation in `try_new`
        unsafe { core::str::from_utf8_unchecked(&grant.deref()[self.topic_name.clone()]) }
    }
}

impl<'a, B: BufferProvider, const SUBS: usize> Message<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    pub fn dup(&self) -> bool {
        self.info.header.dup
    }

    pub fn retain(&self) -> bool {
        self.info.header.retain
    }

    pub fn qos_pid(&self) -> QosPid {
        self.info.qos_pid
    }

    pub fn topic_name(&self) -> &str {
        // # Safety: Checked at instantiation in `try_new`
        unsafe { core::str::from_utf8_unchecked(&self.grant.deref()[self.info.topic_name.clone()]) }
    }

    #[cfg(feature = "mqttv5")]
    pub fn properties(&self) -> crate::Properties<'_> {
        crate::Properties::DataBlock(&self.grant.deref()[self.info.properties.clone()])
    }

    pub fn payload(&self) -> &[u8] {
        // # Safety: Checked at instantiation in `try_new`
        &self.grant.deref()[self.info.payload.clone()]
    }

    pub fn payload_mut(&mut self) -> &mut [u8] {
        // # Safety: Checked at instantiation in `try_new`
        &mut self.grant.deref_mut()[self.info.payload.clone()]
    }
}

impl<'a, B: BufferProvider, const SUBS: usize> Deref for Message<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl<'a, B: BufferProvider, const SUBS: usize> DerefMut for Message<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.payload_mut()
    }
}
