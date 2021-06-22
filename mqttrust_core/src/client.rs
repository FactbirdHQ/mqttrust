use crate::{Mqtt, MqttError, Request};
use core::cell::RefCell;
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
pub struct Client<'a, 'b, const L: usize> {
    client_id: &'b str,
    producer: RefCell<Producer<'a, OwnedRequest, L>>,
}

impl<'a, 'b, const L: usize> Client<'a, 'b, L> {
    pub fn new(producer: Producer<'a, OwnedRequest, L>, client_id: &'b str) -> Self {
        Self {
            client_id,
            producer: RefCell::new(producer),
        }
    }
}

impl<'a, 'b, 'c, const L: usize> Mqtt for Client<'a, 'b, L> {
    fn client_id(&self) -> &str {
        &self.client_id
    }

    fn send(&self, request: Request) -> Result<(), MqttError> {
        self.producer
            .try_borrow_mut()
            .map_err(|_| MqttError::Borrow)?
            .enqueue(request.into())
            .map_err(|_| MqttError::Full)?;
        Ok(())
    }
}

pub mod temp {
    use mqttrs::QoS;
    use mqttrust::{PublishRequest, Request, SubscribeRequest, UnsubscribeRequest};

    pub struct OwnedPublishRequest {
        pub dup: bool,
        pub qos: QoS,
        pub retain: bool,
        pub topic_name: heapless::String<10>,
        pub payload: heapless::Vec<u8, 10>,
    }

    pub enum OwnedRequest {
        Publish(OwnedPublishRequest),
        Subscribe(SubscribeRequest),
        Unsubscribe(UnsubscribeRequest),
        // Reconnect(Connect),
        Disconnect,
    }

    impl OwnedRequest {
        pub fn foo<'a>(&'a self) -> Request<'a> {
            match self {
                OwnedRequest::Publish(p) => Request::Publish(PublishRequest {
                    dup: p.dup,
                    qos: p.qos,
                    retain: p.retain,
                    topic_name: p.topic_name.as_str(),
                    payload: &p.payload,
                }),
                OwnedRequest::Subscribe(s) => Request::Subscribe(s.clone()),
                OwnedRequest::Unsubscribe(u) => Request::Unsubscribe(u.clone()),
                OwnedRequest::Disconnect => Request::Disconnect,
            }
        }
    }

    impl<'a> From<Request<'a>> for OwnedRequest {
        fn from(_: Request<'a>) -> Self {
            todo!()
        }
    }
}
