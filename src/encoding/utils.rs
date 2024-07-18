use core::{convert::TryFrom, num::NonZeroU16};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use super::error::Error;

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
            (0, _) => u16::MAX,
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
            None => Err(Error::BadIdentifier(u)),
        }
    }
}

/// Packet delivery [Quality of Service] level.
///
/// [Quality of Service]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718099
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive, IntoPrimitive, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum QoS {
    /// `QoS 0`. No ack needed.
    AtMostOnce = 0,
    /// `QoS 1`. One ack needed.
    AtLeastOnce = 1,
    /// `QoS 2`. Two acks needed.
    #[cfg(feature = "qos2")]
    ExactlyOnce = 2,
}

/// Combined [`QoS`]/[`Pid`].
///
/// Used only in [`Publish`] packets.
///
/// [`Publish`]: struct.Publish.html
/// [`QoS`]: enum.QoS.html
/// [`Pid`]: struct.Pid.html
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    use crate::encoding::utils::Pid;

    #[test]
    fn pid_add_sub() {
        let t: Vec<(u16, u16, u16, u16)> = vec![
            (2, 1, 1, 3),
            (100, 1, 99, 101),
            (1, 1, u16::MAX, 2),
            (1, 2, u16::MAX - 1, 3),
            (1, 3, u16::MAX - 2, 4),
            (u16::MAX, 1, u16::MAX - 1, 1),
            (u16::MAX, 2, u16::MAX - 2, 2),
            (10, u16::MAX, 10, 10),
            (10, 0, 10, 10),
            (1, 0, 1, 1),
            (u16::MAX, 0, u16::MAX, u16::MAX),
        ];
        for (cur, d, prev, next) in t {
            let sub = Pid::try_from(cur).unwrap() - d;
            let add = Pid::try_from(cur).unwrap() + d;
            assert_eq!(prev, sub.get(), "{} - {} should be {}", cur, d, prev);
            assert_eq!(next, add.get(), "{} + {} should be {}", cur, d, next);
        }
    }
}
