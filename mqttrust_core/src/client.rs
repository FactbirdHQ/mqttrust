use crate::{Mqtt, MqttError, Request};
use core::cell::RefCell;
use core::convert::TryInto;
use heapless::spsc::Producer;

use self::temp::OwnedRequest;

/// MQTT Client
///
/// This client is meerly a convenience wrapper around a
/// `heapless::spsc::Producer`, making it easier to send certain MQTT packet
/// types, and maintaining a common reference to a client id. Also it implements
/// the [`Mqtt`] trait.
///
/// **Lifetimes**:
/// - 'a: The lifetime of the queue exhanging packets between the client and the
///   event loop. This must have the same lifetime as the corresponding
///   Consumer. Usually 'static.
/// - 'b: The lifetime of client id str
///
/// **Generics**:
/// - L: The length of the queue, exhanging packets between the client and the
///   event loop. Length in number of request packets
pub struct Client<'a, 'b, const T: usize, const P: usize, const L: usize> {
    client_id: &'b str,
    producer: RefCell<Producer<'a, OwnedRequest<T, P>, L>>,
}

impl<'a, 'b, const T: usize, const P: usize, const L: usize> Client<'a, 'b, T, P, L> {
    pub fn new(producer: Producer<'a, OwnedRequest<T, P>, L>, client_id: &'b str) -> Self {
        Self {
            client_id,
            producer: RefCell::new(producer),
        }
    }
}

impl<'a, 'b, 'c, const T: usize, const P: usize, const L: usize> Mqtt for Client<'a, 'b, T, P, L> {
    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn send(&self, request: Request) -> Result<(), MqttError> {
        self.producer
            .try_borrow_mut()
            .map_err(|_| MqttError::Borrow)?
            .enqueue(request.try_into().map_err(|_| MqttError::Overflow)?)
            .map_err(|_| MqttError::Full)?;
        Ok(())
    }
}

pub mod temp {
    use core::convert::{TryFrom, TryInto};

    use mqttrs::QoS;
    use mqttrust::{PublishRequest, Request, SubscribeRequest, UnsubscribeRequest};

    #[derive(Debug, Clone)]
    pub struct OwnedPublishRequest<const T: usize, const P: usize> {
        pub dup: bool,
        pub qos: QoS,
        pub retain: bool,
        pub topic_name: heapless::String<T>,
        pub payload: heapless::Vec<u8, P>,
    }

    #[derive(Debug, Clone)]
    pub enum OwnedRequest<const T: usize, const P: usize> {
        Publish(OwnedPublishRequest<T, P>),
        Subscribe(SubscribeRequest),
        Unsubscribe(UnsubscribeRequest),
        // Reconnect(Connect),
        Disconnect,
    }

    impl<'a, const T: usize, const P: usize> TryFrom<Request<'a>> for OwnedRequest<T, P> {
        type Error = ();

        fn try_from(r: Request<'a>) -> Result<Self, Self::Error> {
            Ok(match r {
                Request::Publish(v) => Self::Publish(v.try_into()?),
                Request::Subscribe(v) => Self::Subscribe(v),
                Request::Unsubscribe(v) => Self::Unsubscribe(v),
                Request::Disconnect => Self::Disconnect,
            })
        }
    }

    impl<'a, const T: usize, const P: usize> TryFrom<PublishRequest<'a>> for OwnedPublishRequest<T, P> {
        type Error = ();

        fn try_from(p: PublishRequest<'a>) -> Result<Self, Self::Error> {
            if p.topic_name.len() > T {
                return Err(());
            }

            Ok(Self {
                dup: p.dup,
                qos: p.qos,
                retain: p.retain,
                topic_name: heapless::String::from(p.topic_name),
                payload: heapless::Vec::from_slice(p.payload)?,
            })
        }
    }

    impl<'a, const T: usize, const P: usize> TryFrom<PublishRequest<'a>> for OwnedRequest<T, P> {
        type Error = ();

        fn try_from(p: PublishRequest<'a>) -> Result<Self, Self::Error> {
            Ok(Self::Publish(p.try_into()?))
        }
    }

    impl<const T: usize, const P: usize> From<OwnedPublishRequest<T, P>> for OwnedRequest<T, P> {
        fn from(o: OwnedPublishRequest<T, P>) -> Self {
            Self::Publish(o)
        }
    }

    impl<const T: usize, const P: usize> From<SubscribeRequest> for OwnedRequest<T, P> {
        fn from(s: SubscribeRequest) -> Self {
            Self::Subscribe(s.into())
        }
    }

    impl<const T: usize, const P: usize> From<UnsubscribeRequest> for OwnedRequest<T, P> {
        fn from(u: UnsubscribeRequest) -> Self {
            Self::Unsubscribe(u.into())
        }
    }
}
