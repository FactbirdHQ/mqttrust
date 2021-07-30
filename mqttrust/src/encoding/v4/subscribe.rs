use core::marker::PhantomData;

use super::{decoder::*, encoder::*, *};

/// Subscribe topic.
///
/// [Subscribe] packets contain a `Vec` of those.
///
/// [Subscribe]: struct.Subscribe.html
#[derive(Debug, Clone, PartialEq)]
pub struct SubscribeTopic<'a> {
    pub topic_path: &'a str,
    pub qos: QoS,
}

impl<'a> FromBuffer<'a> for SubscribeTopic<'a> {
    type Item = Self;

    fn from_buffer(buf: &'a [u8], offset: &mut usize) -> Result<Self::Item, Error> {
        let topic_path = read_str(buf, offset)?;
        let qos = QoS::from_u8(buf[*offset])?;
        *offset += 1;
        Ok(SubscribeTopic { topic_path, qos })
    }
}

impl<'a> FromBuffer<'a> for &'a str {
    type Item = Self;

    fn from_buffer(buf: &'a [u8], offset: &mut usize) -> Result<Self::Item, Error> {
        read_str(buf, offset)
    }
}

pub trait FromBuffer<'a> {
    type Item;
    fn from_buffer(buf: &'a [u8], offset: &mut usize) -> Result<Self::Item, Error>;
}

/// Subscribe return value.
///
/// [Suback] packets contain a `Vec` of those.
///
/// [Suback]: struct.Subscribe.html
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeReturnCodes {
    Success(QoS),
    Failure,
}

impl<'a> FromBuffer<'a> for SubscribeReturnCodes {
    type Item = Self;

    fn from_buffer(buf: &'a [u8], offset: &mut usize) -> Result<Self::Item, Error> {
        let code = buf[*offset];
        *offset += 1;

        if code == 0x80 {
            Ok(SubscribeReturnCodes::Failure)
        } else {
            Ok(SubscribeReturnCodes::Success(QoS::from_u8(code)?))
        }
    }
}

impl SubscribeReturnCodes {
    pub(crate) fn to_u8(&self) -> u8 {
        match *self {
            SubscribeReturnCodes::Failure => 0x80,
            SubscribeReturnCodes::Success(qos) => qos.to_u8(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum List<'a, T> {
    Owned(&'a [T]),
    Lazy(LazyList<'a, T>),
}

impl<'a, T> List<'a, T>
where
    T: FromBuffer<'a, Item = T>,
{
    pub fn len(&self) -> usize {
        match self {
            List::Owned(data) => data.len(),
            List::Lazy(data) => {
                let mut len = 0;
                let mut offset = 0;
                while T::from_buffer(data.0, &mut offset).is_ok() {
                    len += 1;
                }
                len
            }
        }
    }
}

impl<'a, T> IntoIterator for &'a List<'a, T>
where
    T: FromBuffer<'a, Item = T> + Clone,
{
    type Item = T;

    type IntoIter = ListIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        ListIter {
            list: self,
            index: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LazyList<'a, T>(&'a [u8], PhantomData<T>);

pub struct ListIter<'a, T> {
    list: &'a List<'a, T>,
    index: usize,
}

impl<'a, T> Iterator for ListIter<'a, T>
where
    T: FromBuffer<'a, Item = T> + Clone,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match self.list {
            List::Owned(data) => {
                // FIXME: Can we get rid of this clone?
                let item = data.get(self.index).map(|t| t.clone());
                self.index += 1;
                item
            }
            List::Lazy(data) => T::from_buffer(data.0, &mut self.index).ok(),
        }
    }
}

/// Subscribe packet ([MQTT 3.8]).
///
/// [MQTT 3.8]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718063
#[derive(Debug, Clone, PartialEq)]
pub struct Subscribe<'a> {
    pid: Option<Pid>,
    topics: List<'a, SubscribeTopic<'a>>,
}

/// Subsack packet ([MQTT 3.9]).
///
/// [MQTT 3.9]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718068
#[derive(Debug, Clone, PartialEq)]
pub struct Suback<'a> {
    pub pid: Pid,
    pub return_codes: &'a [SubscribeReturnCodes],
}

/// Unsubscribe packet ([MQTT 3.10]).
///
/// [MQTT 3.10]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718072
#[derive(Debug, Clone, PartialEq)]
pub struct Unsubscribe<'a> {
    pub pid: Option<Pid>,
    pub topics: List<'a, &'a str>,
}

impl<'a> Subscribe<'a> {
    pub fn new(topics: &'a [SubscribeTopic<'a>]) -> Self {
        Self {
            pid: None,
            topics: List::Owned(topics),
        }
    }

    pub fn topics(&self) -> impl Iterator<Item = SubscribeTopic<'_>> {
        self.topics.into_iter()
    }

    pub fn pid(&self) -> Option<Pid> {
        self.pid
    }

    pub(crate) fn from_buffer(
        remaining_len: usize,
        buf: &'a [u8],
        offset: &mut usize,
    ) -> Result<Self, Error> {
        let payload_end = *offset + remaining_len;
        let pid = Pid::from_buffer(buf, offset)?;

        Ok(Subscribe {
            pid: Some(pid),
            topics: List::Lazy(LazyList(&buf[*offset..payload_end], PhantomData)),
        })
    }

    pub(crate) fn to_buffer(&self, buf: &mut [u8], offset: &mut usize) -> Result<usize, Error> {
        let header: u8 = 0b10000010;
        check_remaining(buf, offset, 1)?;
        write_u8(buf, offset, header)?;

        // Length: pid(2) + topic.for_each(2+len + qos(1))
        let mut length = 2;
        for topic in self.topics() {
            length += topic.topic_path.len() + 2 + 1;
        }
        let write_len = write_length(buf, offset, length)? + 1;

        // Pid
        self.pid.unwrap_or(Pid::default()).to_buffer(buf, offset)?;

        // Topics
        for topic in self.topics() {
            write_string(buf, offset, topic.topic_path)?;
            write_u8(buf, offset, topic.qos.to_u8())?;
        }

        Ok(write_len)
    }
}

impl<'a> Unsubscribe<'a> {
    pub fn new(topics: &'a [&'a str]) -> Self {
        Self {
            pid: None,
            topics: List::Owned(topics),
        }
    }

    pub fn topics(&self) -> impl Iterator<Item = &str> {
        self.topics.into_iter()
    }

    pub fn pid(&self) -> Option<Pid> {
        self.pid
    }

    pub(crate) fn from_buffer(
        remaining_len: usize,
        buf: &'a [u8],
        offset: &mut usize,
    ) -> Result<Self, Error> {
        let payload_end = *offset + remaining_len;
        let pid = Pid::from_buffer(buf, offset)?;

        Ok(Unsubscribe {
            pid: Some(pid),
            topics: List::Lazy(LazyList(&buf[*offset..payload_end], PhantomData)),
        })
    }

    pub(crate) fn to_buffer(&self, buf: &mut [u8], offset: &mut usize) -> Result<usize, Error> {
        let header: u8 = 0b10100010;
        let mut length = 2;
        for topic in self.topics() {
            length += 2 + topic.len();
        }
        check_remaining(buf, offset, 1)?;
        write_u8(buf, offset, header)?;

        let write_len = write_length(buf, offset, length)? + 1;

        // Pid
        self.pid.unwrap_or(Pid::default()).to_buffer(buf, offset)?;

        for topic in self.topics() {
            write_string(buf, offset, topic)?;
        }
        Ok(write_len)
    }
}

impl<'a> Suback<'a> {
    pub(crate) fn from_buffer(
        remaining_len: usize,
        buf: &'a [u8],
        offset: &mut usize,
    ) -> Result<Self, Error> {
        let payload_end = *offset + remaining_len;
        let pid = Pid::from_buffer(buf, offset)?;

        // let mut return_codes = LimitedVec::new();
        // while *offset < payload_end {
        //     let _res = return_codes.push(SubscribeReturnCodes::from_buffer(buf, offset)?);

        //     #[cfg(not(feature = "std"))]
        //     _res.map_err(|_| Error::InvalidLength)?;
        // }

        Ok(Suback {
            pid,
            return_codes: &[],
        })
    }

    pub(crate) fn to_buffer(&self, buf: &mut [u8], offset: &mut usize) -> Result<usize, Error> {
        let header: u8 = 0b10010000;
        let length = 2 + self.return_codes.len();
        check_remaining(buf, offset, 1)?;
        write_u8(buf, offset, header)?;

        let write_len = write_length(buf, offset, length)? + 1;
        self.pid.to_buffer(buf, offset)?;
        for rc in self.return_codes {
            write_u8(buf, offset, rc.to_u8())?;
        }
        Ok(write_len)
    }
}
