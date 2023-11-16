use super::encoder::write_u16;
use core::{convert::TryFrom, fmt, num::NonZeroU16};

#[cfg(feature = "derive")]
use serde::{Deserialize, Serialize};

/// Errors returned by [`encode()`] and [`decode()`].
///
/// [`encode()`]: fn.encode.html
/// [`decode()`]: fn.decode.html
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    /// Not enough space in the write buffer.
    ///
    /// It is the caller's responsiblity to pass a big enough buffer to `encode()`.
    WriteZero,
    /// Tried to encode or decode a ProcessIdentifier==0.
    InvalidPid(u16),
    /// Tried to decode a QoS > 2.
    InvalidQos(u8),
    /// Tried to decode a ConnectReturnCode > 5.
    InvalidConnectReturnCode(u8),
    /// Tried to decode an unknown protocol.
    InvalidProtocol(heapless::String<10>, u8),
    /// Tried to decode an invalid fixed header (packet type, flags, or remaining_length).
    InvalidHeader,
    /// Trying to encode/decode an invalid length.
    ///
    /// The difference with `WriteZero`/`UnexpectedEof` is that it refers to an invalid/corrupt
    /// length rather than a buffer size issue.
    InvalidLength,
    /// Trying to decode a non-utf8 string.
    InvalidString,

    PidMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// Io Error while state is passed to network
    Io(embedded_io_async::ErrorKind),
    /// Invalid state for a given operation
    InvalidState,
    /// Received a packet (ack) which isn't asked for
    Unsolicited(u16),
    /// Last pingreq isn't acked
    AwaitPingResp,
    /// Received a wrong packet while waiting for another packet
    WrongPacket,
    CollisionTimeout,
    EmptySubscription,
    Deserialization,
    OutgoingPacketTooLarge {
        pkt_size: usize,
        max: usize,
    },
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Io(kind) => write!(f, "IO error: {kind:?}"),
            Self::InvalidState => write!(f, "Invalid state for a given operation"),
            Self::Unsolicited(pid) => write!(f, "Received unsolicited ack pkid: {pid}"),
            Self::AwaitPingResp => write!(f, "Last pingreq isn't acked"),
            Self::WrongPacket => write!(f, "Received a wrong packet while waiting for another packet"),
            Self::CollisionTimeout => write!(f, "Timeout while waiting to resolve collision"),
            Self::EmptySubscription => write!(f, "A Subscribe packet must contain atleast one filter"),
            Self::Deserialization => write!(f, "Mqtt serialization/deserialization error"),
            Self::OutgoingPacketTooLarge { pkt_size, max} => write!(f, "Cannot recieve packet of size '{pkt_size:?}'. It's greater than the client's maximum packet size of: '{max:?}'"),
        }
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for StateError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Io(kind) => defmt::write!(f, "IO error: {:?}", kind),
            Self::InvalidState => defmt::write!(f, "Invalid state for a given operation"),
            Self::Unsolicited(pid) => defmt::write!(f, "Received unsolicited ack pkid: {}", pid),
            Self::AwaitPingResp => defmt::write!(f, "Last pingreq isn't acked"),
            Self::WrongPacket => defmt::write!(f, "Received a wrong packet while waiting for another packet"),
            Self::CollisionTimeout => defmt::write!(f, "Timeout while waiting to resolve collision"),
            Self::EmptySubscription => defmt::write!(f, "A Subscribe packet must contain atleast one filter"),
            Self::Deserialization => defmt::write!(f, "Mqtt serialization/deserialization error"),
            Self::OutgoingPacketTooLarge { pkt_size, max} => defmt::write!(f, "Cannot recieve packet of size '{:?}'. It's greater than the client's maximum packet size of: '{:?}'", pkt_size, max),
        }
    }
}

/// Packet Identifier.
///
/// For packets with [`QoS::AtLeastOne` or `QoS::ExactlyOnce`] delivery.
///
/// ```rust
/// # use mqttrust::encoding::v4::{Packet, Pid, QosPid};
/// # use std::convert::TryFrom;
/// #[derive(Default)]
/// struct Session {
///    pid: Pid,
/// }
/// impl Session {
///    pub fn next_pid(&mut self) -> Pid {
///        self.pid = self.pid + 1;
///        self.pid
///    }
/// }
///
/// let mut sess = Session::default();
/// assert_eq!(2, sess.next_pid().get());
/// assert_eq!(Pid::try_from(3).unwrap(), sess.next_pid());
/// ```
///
/// The spec ([MQTT-2.3.1-1], [MQTT-2.2.1-3]) disallows a pid of 0.
///
/// [`QoS::AtLeastOne` or `QoS::ExactlyOnce`]: enum.QoS.html
/// [MQTT-2.3.1-1]: https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718025
/// [MQTT-2.2.1-3]: https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html#_Toc3901026
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "derive", derive(Serialize, Deserialize))]
pub struct Pid(NonZeroU16);
impl Pid {
    /// Returns a new `Pid` with value `1`.
    pub fn new() -> Self {
        Pid(NonZeroU16::new(1).unwrap())
    }

    /// Get the `Pid` as a raw `u16`.
    pub fn get(self) -> u16 {
        self.0.get()
    }

    pub(crate) fn from_buffer(buf: &[u8], offset: &mut usize) -> Result<Self, Error> {
        let pid = ((buf[*offset] as u16) << 8) | buf[*offset + 1] as u16;
        *offset += 2;
        Self::try_from(pid)
    }

    pub fn to_buffer(self, buf: &mut [u8], offset: &mut usize) -> Result<(), Error> {
        write_u16(buf, offset, self.get())
    }
}

impl Default for Pid {
    fn default() -> Pid {
        Pid::new()
    }
}

impl core::ops::Add<u16> for Pid {
    type Output = Pid;

    /// Adding a `u16` to a `Pid` will wrap around and avoid 0.
    fn add(self, u: u16) -> Pid {
        let n = match self.get().overflowing_add(u) {
            (n, false) => n,
            (n, true) => n + 1,
        };
        Pid(NonZeroU16::new(n).unwrap())
    }
}

impl core::ops::Sub<u16> for Pid {
    type Output = Pid;

    /// Adding a `u16` to a `Pid` will wrap around and avoid 0.
    fn sub(self, u: u16) -> Pid {
        let n = match self.get().overflowing_sub(u) {
            (0, _) => core::u16::MAX,
            (n, false) => n,
            (n, true) => n - 1,
        };
        Pid(NonZeroU16::new(n).unwrap())
    }
}

impl From<Pid> for u16 {
    /// Convert `Pid` to `u16`.
    fn from(p: Pid) -> Self {
        p.0.get()
    }
}

impl TryFrom<u16> for Pid {
    type Error = Error;

    /// Convert `u16` to `Pid`. Will fail for value 0.
    fn try_from(u: u16) -> Result<Self, Error> {
        match NonZeroU16::new(u) {
            Some(nz) => Ok(Pid(nz)),
            None => Err(Error::InvalidPid(u)),
        }
    }
}

/// Packet delivery [Quality of Service] level.
///
/// [Quality of Service]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718099
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "derive", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum QoS {
    /// `QoS 0`. No ack needed.
    AtMostOnce,
    /// `QoS 1`. One ack needed.
    AtLeastOnce,
    /// `QoS 2`. Two acks needed.
    #[cfg(feature = "qos2")]
    ExactlyOnce,
}

impl QoS {
    pub(crate) fn as_u8(&self) -> u8 {
        match *self {
            QoS::AtMostOnce => 0,
            QoS::AtLeastOnce => 1,
            #[cfg(feature = "qos2")]
            QoS::ExactlyOnce => 2,
        }
    }

    pub(crate) fn from_u8(byte: u8) -> Result<QoS, Error> {
        match byte {
            0 => Ok(QoS::AtMostOnce),
            1 => Ok(QoS::AtLeastOnce),
            #[cfg(feature = "qos2")]
            2 => Ok(QoS::ExactlyOnce),
            n => Err(Error::InvalidQos(n)),
        }
    }
}

/// Combined [`QoS`]/[`Pid`].
///
/// Used only in [`Publish`] packets.
///
/// [`Publish`]: struct.Publish.html
/// [`QoS`]: enum.QoS.html
/// [`Pid`]: struct.Pid.html
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "derive", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum QosPid {
    AtMostOnce,
    AtLeastOnce(Pid),
    #[cfg(feature = "qos2")]
    ExactlyOnce(Pid),
}

impl QosPid {
    /// Extract the [`Pid`] from a `QosPid`, if any.
    ///
    /// [`Pid`]: struct.Pid.html
    pub fn pid(self) -> Option<Pid> {
        match self {
            QosPid::AtMostOnce => None,
            QosPid::AtLeastOnce(p) => Some(p),
            #[cfg(feature = "qos2")]
            QosPid::ExactlyOnce(p) => Some(p),
        }
    }

    /// Extract the [`QoS`] from a `QosPid`.
    ///
    /// [`QoS`]: enum.QoS.html
    pub fn qos(self) -> QoS {
        match self {
            QosPid::AtMostOnce => QoS::AtMostOnce,
            QosPid::AtLeastOnce(_) => QoS::AtLeastOnce,
            #[cfg(feature = "qos2")]
            QosPid::ExactlyOnce(_) => QoS::ExactlyOnce,
        }
    }
}

#[cfg(test)]
mod test {
    use core::convert::TryFrom;
    use std::vec;

    use crate::encoding::Pid;

    #[test]
    fn pid_add_sub() {
        let t: Vec<(u16, u16, u16, u16)> = vec![
            (2, 1, 1, 3),
            (100, 1, 99, 101),
            (1, 1, core::u16::MAX, 2),
            (1, 2, core::u16::MAX - 1, 3),
            (1, 3, core::u16::MAX - 2, 4),
            (core::u16::MAX, 1, core::u16::MAX - 1, 1),
            (core::u16::MAX, 2, core::u16::MAX - 2, 2),
            (10, core::u16::MAX, 10, 10),
            (10, 0, 10, 10),
            (1, 0, 1, 1),
            (core::u16::MAX, 0, core::u16::MAX, core::u16::MAX),
        ];
        for (cur, d, prev, next) in t {
            let sub = Pid::try_from(cur).unwrap() - d;
            let add = Pid::try_from(cur).unwrap() + d;
            assert_eq!(prev, sub.get(), "{} - {} should be {}", cur, d, prev);
            assert_eq!(next, add.get(), "{} + {} should be {}", cur, d, next);
        }
    }
}
