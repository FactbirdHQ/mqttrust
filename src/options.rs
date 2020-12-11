use mqttrs::LastWill;
use no_std_net::{IpAddr, Ipv4Addr};

#[derive(Clone, Debug, PartialEq)]
pub enum Broker<'a> {
    Hostname(&'a str),
    IpAddr(IpAddr),
}

impl<'a> From<&'a str> for Broker<'a> {
    fn from(s: &'a str) -> Self {
        Broker::Hostname(s)
    }
}

impl<'a> From<IpAddr> for Broker<'a> {
    fn from(ip: IpAddr) -> Self {
        Broker::IpAddr(ip)
    }
}

impl<'a> From<Ipv4Addr> for Broker<'a> {
    fn from(ip: Ipv4Addr) -> Self {
        Broker::IpAddr(ip.into())
    }
}

type Certificate<'a> = &'a [u8];
type PrivateKey<'a> = &'a [u8];
type Password<'a> = &'a [u8];

/// Options to configure the behaviour of mqtt connection
///
/// **Lifetimes**:
/// - 'a: The lifetime of option fields, not referenced in any MQTT packets at any point
/// - 'b: The lifetime of the packet fields, backed by a slice buffer
#[derive(Clone, Debug)]
pub struct MqttOptions<'a> {
    /// broker address that you want to connect to
    broker_addr: Broker<'a>,
    /// broker port
    port: u16,
    /// keep alive time to send pingreq to broker when the connection is idle
    keep_alive_ms: u32,
    /// clean (or) persistent session
    clean_session: bool,
    /// client identifier
    client_id: &'a str,
    /// certificate authority certificate
    ca: Option<&'a [u8]>,
    /// tls client_authentication
    client_auth: Option<(Certificate<'a>, PrivateKey<'a>, Option<Password<'a>>)>,
    /// alpn settings
    // alpn: Option<Vec<Vec<u8>>>,
    /// username and password
    credentials: Option<(&'a str, &'a [u8])>,
    /// Minimum delay time between consecutive outgoing packets
    // throttle: Duration,
    /// maximum number of outgoing inflight messages
    inflight: usize,
    /// Last will that will be issued on unexpected disconnect
    last_will: Option<LastWill<'a>>,
}

impl<'a> MqttOptions<'a> {
    /// New mqtt options
    pub fn new(id: &'a str, broker: Broker<'a>, port: u16) -> MqttOptions<'a> {
        if id.starts_with(' ') || id.is_empty() {
            panic!("Invalid client id")
        }

        MqttOptions {
            broker_addr: broker,
            port,
            keep_alive_ms: 60_000,
            clean_session: true,
            client_id: id,
            ca: None,
            client_auth: None,
            // alpn: None,
            credentials: None,
            // throttle: Duration::from_micros(0),
            inflight: 3,
            last_will: None,
        }
    }

    /// Broker address
    pub fn broker(&self) -> (Broker, u16) {
        (self.broker_addr.clone(), self.port)
    }

    pub fn set_last_will(self, will: LastWill<'a>) -> Self {
        Self {
            last_will: Some(will),
            ..self
        }
    }

    pub fn last_will(&self) -> Option<LastWill<'a>> {
        self.last_will.clone()
    }

    pub fn set_ca(self, ca: &'a [u8]) -> Self {
        Self {
            ca: Some(ca),
            ..self
        }
    }

    pub fn ca(&self) -> Option<&[u8]> {
        self.ca
    }

    pub fn set_client_auth(
        self,
        cert: Certificate<'a>,
        key: PrivateKey<'a>,
        password: Option<Password<'a>>,
    ) -> Self {
        Self {
            client_auth: Some((cert, key, password)),
            ..self
        }
    }

    pub fn client_auth(&self) -> Option<(Certificate<'a>, PrivateKey<'a>, Option<Password<'a>>)> {
        self.client_auth
    }

    // pub fn set_alpn(self, alpn: Vec<Vec<u8>>) -> Self {
    //     Self {
    //         alpn: Some(alpn),
    //         ..self
    //     }
    // }

    // pub fn alpn(&self) -> Option<Vec<Vec<u8>>> {
    //     self.alpn.clone()
    // }

    /// Set number of seconds after which client should ping the broker
    /// if there is no other data exchange
    pub fn set_keep_alive(self, secs: u16) -> Self {
        if secs < 5 {
            panic!("Keep alives should be >= 5  secs");
        }

        Self {
            keep_alive_ms: secs as u32 * 1000,
            ..self
        }
    }

    /// Keep alive time
    pub fn keep_alive_ms(&self) -> u32 {
        self.keep_alive_ms
    }

    /// Client identifier
    pub fn client_id(&self) -> &'a str {
        self.client_id
    }

    /// `clean_session = true` removes all the state from queues & instructs the broker
    /// to clean all the client state when client disconnects.
    ///
    /// When set `false`, broker will hold the client state and performs pending
    /// operations on the client when reconnection with same `client_id`
    /// happens. Local queue state is also held to retransmit packets after reconnection.
    pub fn set_clean_session(self, clean_session: bool) -> Self {
        Self {
            clean_session,
            ..self
        }
    }

    /// Clean session
    pub fn clean_session(&self) -> bool {
        self.clean_session
    }

    /// Username and password
    pub fn set_credentials(self, username: &'a str, password: &'a [u8]) -> Self {
        Self {
            credentials: Some((username, password)),
            ..self
        }
    }

    /// Security options
    pub fn credentials(&self) -> (Option<&'a str>, Option<&'a [u8]>) {
        if let Some((username, password)) = self.credentials {
            (Some(username), Some(password))
        } else {
            (None, None)
        }
    }

    // /// Enables throttling and sets outoing message rate to the specified 'rate'
    // pub fn set_throttle(self, duration: Duration) -> Self {
    //     self.throttle = duration;
    //     self
    // }

    // /// Outgoing message rate
    // pub fn throttle(&self) -> Duration {
    //     self.throttle
    // }

    /// Set number of concurrent in flight messages
    pub fn set_inflight(self, inflight: usize) -> Self {
        if inflight == 0 {
            panic!("zero in flight is not allowed")
        }

        Self { inflight, ..self }
    }

    /// Number of concurrent in flight messages
    pub fn inflight(&self) -> usize {
        self.inflight
    }
}

#[cfg(test)]
mod test {
    use super::{Ipv4Addr, MqttOptions};
    use embedded_nal::{IpAddr, Ipv6Addr};
    use mqttrs::LastWill;

    #[test]
    #[should_panic]
    fn client_id_starts_with_space() {
        let _mqtt_opts = MqttOptions::new(" client_a", Ipv4Addr::new(127, 0, 0, 1).into(), 1883)
            .set_clean_session(true);
    }

    #[test]
    #[should_panic]
    fn no_client_id() {
        let _mqtt_opts =
            MqttOptions::new("", Ipv4Addr::localhost().into(), 1883).set_clean_session(true);
    }

    #[test]
    fn broker() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.broker_addr, Ipv4Addr::localhost().into());
        assert_eq!(opts.port, 1883);
        assert_eq!(opts.broker(), (Ipv4Addr::localhost().into(), 1883));
        assert_eq!(
            MqttOptions::new("client_a", "localhost".into(), 1883).broker_addr,
            "localhost".into()
        );
        assert_eq!(
            MqttOptions::new("client_a", IpAddr::V4(Ipv4Addr::localhost()).into(), 1883)
                .broker_addr,
            IpAddr::V4(Ipv4Addr::localhost()).into()
        );
        assert_eq!(
            MqttOptions::new("client_a", IpAddr::V6(Ipv6Addr::localhost()).into(), 1883)
                .broker_addr,
            IpAddr::V6(Ipv6Addr::localhost()).into()
        );
    }

    #[test]
    fn client_id() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.client_id(), "client_a");
    }

    #[test]
    fn inflight() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.inflight, 3);
        assert_eq!(opts.set_inflight(5).inflight(), 5);
    }

    #[test]
    #[should_panic]
    fn zero_inflight() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.inflight, 3);
        assert_eq!(opts.set_inflight(0).inflight(), 5);
    }

    #[test]
    fn client_auth() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.client_auth, None);
        assert_eq!(
            opts.clone()
                .set_client_auth(b"Certificate", b"PrivateKey", None)
                .client_auth(),
            Some((&b"Certificate"[..], &b"PrivateKey"[..], None))
        );
        assert_eq!(
            opts.set_client_auth(b"Certificate", b"PrivateKey", Some(b"Password"))
                .client_auth(),
            Some((
                &b"Certificate"[..],
                &b"PrivateKey"[..],
                Some(&b"Password"[..])
            ))
        );
    }

    #[test]
    fn ca() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.ca, None);
        assert_eq!(
            opts.set_ca(b"My Certificate Authority").ca(),
            Some(&b"My Certificate Authority"[..])
        );
    }

    #[test]
    fn keep_alive_ms() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.keep_alive_ms, 60_000);
        assert_eq!(opts.set_keep_alive(120).keep_alive_ms(), 120_000);
    }

    #[test]
    #[should_panic]
    fn keep_alive_panic() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.keep_alive_ms, 60_000);
        assert_eq!(opts.set_keep_alive(4).keep_alive_ms(), 120_000);
    }

    #[test]
    fn last_will() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.last_will, None);
        let will = LastWill {
            topic: "topic",
            message: b"Will message",
            qos: mqttrs::QoS::AtLeastOnce,
            retain: false,
        };
        assert_eq!(opts.set_last_will(will.clone()).last_will(), Some(will));
    }

    #[test]
    fn clean_session() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.clean_session, true);
        assert_eq!(opts.set_clean_session(false).clean_session(), false);
    }

    #[test]
    fn credentials() {
        let opts = MqttOptions::new("client_a", Ipv4Addr::localhost().into(), 1883);
        assert_eq!(opts.credentials, None);
        assert_eq!(opts.credentials(), (None, None));
        assert_eq!(
            opts.set_credentials("some_user", &[]).credentials(),
            (Some("some_user"), Some(&b""[..]))
        );
    }
}
