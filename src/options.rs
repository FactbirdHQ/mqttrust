use embedded_nal::HostSocketAddr;
use mqttrs::LastWill;

#[derive(Clone, Debug)]
pub struct MqttOptionsBuilder<'a>(MqttOptions<'a, ()>);

#[allow(dead_code)]
impl<'a> MqttOptionsBuilder<'a> {
    /// New mqtt options builder
    pub fn new(id: &'a str, broker: HostSocketAddr) -> Self {
        if id.starts_with(' ') || id.is_empty() {
            panic!("Invalid client id")
        }

        Self(MqttOptions {
            broker,
            keep_alive_ms: 60_000,
            clean_session: true,
            client_id: id,
            tls_connector: None,
            credentials: None,
            // throttle: Duration::from_micros(0),
            inflight: 3,
            last_will: None,
        })
    }

    pub fn set_last_will(self, will: LastWill<'a>) -> Self {
        Self(MqttOptions {
            last_will: Some(will),
            ..self.0
        })
    }

    /// Set number of seconds after which client should ping the broker
    /// if there is no other data exchange
    pub fn set_keep_alive(self, secs: u16) -> Self {
        if secs < 5 {
            panic!("Keep alives should be >= 5  secs");
        }

        Self(MqttOptions {
            keep_alive_ms: secs as u32 * 1000,
            ..self.0
        })
    }

    /// `clean_session = true` removes all the state from queues & instructs the broker
    /// to clean all the client state when client disconnects.
    ///
    /// When set `false`, broker will hold the client state and performs pending
    /// operations on the client when reconnection with same `client_id`
    /// happens. Local queue state is also held to retransmit packets after reconnection.
    pub fn set_clean_session(self, clean_session: bool) -> Self {
        Self(MqttOptions {
            clean_session,
            ..self.0
        })
    }

    /// Username and password
    pub fn set_credentials(self, username: &'a str, password: &'a [u8]) -> Self {
        Self(MqttOptions {
            credentials: Some((username, password)),
            ..self.0
        })
    }

    // /// Enables throttling and sets outoing message rate to the specified 'rate'
    // pub fn set_throttle(self, duration: Duration) -> Self {
    //     self.0.throttle = duration;
    //     self
    // }

    /// Set number of concurrent in flight messages
    pub fn set_inflight(self, inflight: usize) -> Self {
        if inflight == 0 {
            panic!("zero in flight is not allowed")
        }

        Self(MqttOptions { inflight, ..self.0 })
    }

    pub fn build(self) -> MqttOptions<'a, ()> {
        self.0
    }

    pub fn build_with_tls<T>(self, tls_connector: T) -> MqttOptions<'a, T> {
        MqttOptions {
            broker: self.0.broker,
            keep_alive_ms: self.0.keep_alive_ms,
            clean_session: self.0.clean_session,
            client_id: self.0.client_id,
            credentials: self.0.credentials,
            inflight: self.0.inflight,
            last_will: self.0.last_will,
            tls_connector: Some(tls_connector),
        }
    }
}

/// Options to configure the behaviour of mqtt connection
///
/// **Lifetimes**:
/// - 'a: The lifetime of option fields, not referenced in any MQTT packets at any point
/// - 'b: The lifetime of the packet fields, backed by a slice buffer
#[derive(Clone, Debug)]
pub struct MqttOptions<'a, T> {
    /// broker address that you want to connect to
    broker: HostSocketAddr,
    /// keep alive time to send pingreq to broker when the connection is idle
    keep_alive_ms: u32,
    /// clean (or) persistent session
    clean_session: bool,
    /// client identifier
    client_id: &'a str,
    tls_connector: Option<T>,
    /// username and password
    credentials: Option<(&'a str, &'a [u8])>,
    /// Minimum delay time between consecutive outgoing packets
    // throttle: Duration,
    /// maximum number of outgoing inflight messages
    inflight: usize,
    /// Last will that will be issued on unexpected disconnect
    last_will: Option<LastWill<'a>>,
}

impl<'a, T> MqttOptions<'a, T> {
    /// Broker address
    pub fn broker(&self) -> HostSocketAddr {
        self.broker.clone()
    }

    pub fn last_will(&self) -> Option<LastWill<'a>> {
        self.last_will.clone()
    }

    pub fn tls_connector(&self) -> &Option<T> {
        &self.tls_connector
    }

    /// Keep alive time
    pub fn keep_alive_ms(&self) -> u32 {
        self.keep_alive_ms
    }

    /// Client identifier
    pub fn client_id(&self) -> &'a str {
        self.client_id
    }

    /// Clean session
    pub fn clean_session(&self) -> bool {
        self.clean_session
    }

    /// Security options
    pub fn credentials(&self) -> (Option<&'a str>, Option<&'a [u8]>) {
        if let Some((username, password)) = self.credentials {
            (Some(username), Some(password))
        } else {
            (None, None)
        }
    }

    // /// Outgoing message rate
    // pub fn throttle(&self) -> Duration {
    //     self.throttle
    // }

    /// Number of concurrent in flight messages
    pub fn inflight(&self) -> usize {
        self.inflight
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use embedded_nal::Ipv4Addr;

    #[test]
    #[should_panic]
    fn client_id_startswith_space() {
        let builder = MqttOptionsBuilder::new(
            " client_a",
            HostSocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 1883),
        )
        .set_clean_session(true)
        .build();
    }

    #[test]
    #[should_panic]
    fn no_client_id() {
        MqttOptionsBuilder::new("", HostSocketAddr::new(Ipv4Addr::localhost().into(), 1883))
            .set_clean_session(true)
            .build();
    }
}
