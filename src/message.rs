use core::ops::{Deref, DerefMut, Range};

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{
    crate_config::MAX_SUBSCRIBERS,
    decoder::{FixedHeader, MqttDecoder},
    BufferProvider, FrameGrantR, Pid, QoS, QosPid,
};

/// Represents an MQTT message with associated metadata and payload.
pub struct Message<'a, M: RawMutex, B: BufferProvider> {
    /// The grant for the frame, which includes the buffer and metadata.
    grant: FrameGrantR<'a, M, B, MAX_SUBSCRIBERS>,
    /// The fixed header of the MQTT message.
    header: FixedHeader,
    /// The QoS and Packet Identifier of the message.
    qos_pid: QosPid,
    /// The range within the buffer that contains the topic name.
    topic_name: Range<usize>,
    /// The range within the buffer that contains the payload.
    payload: Range<usize>,
    /// The range within the buffer that contains the properties (MQTT v5 only).
    #[cfg(feature = "mqttv5")]
    properties: Range<usize>,
}

impl<'a, M: RawMutex, B: BufferProvider> Message<'a, M, B> {
    /// Tries to create a new `Message` from a given frame grant.
    ///
    /// Returns `Some(Message)` if successful, otherwise `None`.
    pub(crate) fn try_new(
        grant: FrameGrantR<'a, M, B, MAX_SUBSCRIBERS>,
    ) -> Result<Self, crate::encoding::EncodingError> {
        let mut decoder = MqttDecoder::try_new(grant.deref())?;

        let header = decoder.fixed_header();
        decoder.check_remaining(header.remaining_len)?;

        let topic_name = {
            let topic_name_start = decoder.offset() + 2;
            let _name = decoder.read_str()?;
            topic_name_start..decoder.offset()
        };

        let qos_pid = match header.qos {
            QoS::AtMostOnce => QosPid::AtMostOnce,
            QoS::AtLeastOnce => QosPid::AtLeastOnce(Pid::try_from(decoder.read_u16()?)?),
            #[cfg(feature = "qos2")]
            QoS::ExactlyOnce => QosPid::ExactlyOnce(Pid::try_from(decoder.read_u16()?)?),
        };

        #[cfg(feature = "mqttv5")]
        let properties = {
            let properties_start = decoder.offset();
            decoder.read_properties()?;
            properties_start..decoder.offset()
        };

        let payload = {
            let payload_start = decoder.offset();
            decoder.read_payload()?;
            payload_start..decoder.offset()
        };

        Ok(Self {
            grant,
            header,
            qos_pid,
            topic_name,
            #[cfg(feature = "mqttv5")]
            properties,
            payload,
        })
    }

    /// Returns whether the message is a duplicate.
    pub fn dup(&self) -> bool {
        self.header.dup
    }

    /// Returns whether the message should be retained.
    pub fn retain(&self) -> bool {
        self.header.retain
    }

    /// Returns the QoS and Packet Identifier of the message.
    pub fn qos_pid(&self) -> QosPid {
        self.qos_pid
    }

    /// Returns the topic name of the message as a string slice.
    pub fn topic_name(&self) -> &str {
        // # Safety: Checked at instantiation in `try_new`
        unsafe { core::str::from_utf8_unchecked(&self.grant.deref()[self.topic_name.clone()]) }
    }

    /// Returns the properties of the message (MQTT v5 only).
    #[cfg(feature = "mqttv5")]
    pub fn properties(&self) -> crate::Properties<'_> {
        crate::Properties::DataBlock(&self.grant.deref()[self.properties.clone()])
    }

    /// Returns the payload of the message as a byte slice.
    pub fn payload(&self) -> &[u8] {
        // # Safety: Checked at instantiation in `try_new`
        &self.grant.deref()[self.payload.clone()]
    }

    /// Returns the payload of the message as a mutable byte slice.
    pub fn payload_mut(&mut self) -> &mut [u8] {
        // # Safety: Checked at instantiation in `try_new`
        &mut self.grant.deref_mut()[self.payload.clone()]
    }
}

impl<'a, M: RawMutex, B: BufferProvider> Deref for Message<'a, M, B> {
    type Target = [u8];

    /// Dereferences the message to its payload.
    fn deref(&self) -> &Self::Target {
        self.payload()
    }
}

impl<'a, M: RawMutex, B: BufferProvider> DerefMut for Message<'a, M, B> {
    /// Dereferences the message to its mutable payload.
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.payload_mut()
    }
}
