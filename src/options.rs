use mqttrs::LastWill;
use no_std_net::{IpAddr, Ipv4Addr};

use alloc::string::String;

#[derive(Clone, Debug)]
pub enum Broker {
    Hostname(String),
    IpAddr(IpAddr),
}

impl<'a> From<&'a str> for Broker {
    fn from(s: &'a str) -> Self {
        Broker::Hostname(String::from(s))
    }
}

impl<'a> From<String> for Broker {
    fn from(s: String) -> Self {
        Broker::Hostname(s)
    }
}

impl<'a> From<IpAddr> for Broker {
    fn from(ip: IpAddr) -> Self {
        Broker::IpAddr(ip)
    }
}

impl<'a> From<Ipv4Addr> for Broker {
    fn from(ip: Ipv4Addr) -> Self {
        Broker::IpAddr(ip.into())
    }
}

type Certificate<'a> = &'a [u8];
type PrivateKey<'a> = &'a [u8];
type Password<'a> = &'a [u8];

/// Options to configure the behaviour of mqtt connection
#[derive(Clone, Debug)]
pub struct MqttOptions<'a> {
    /// broker address that you want to connect to
    broker_addr: Broker,
    /// broker port
    port: u16,
    /// keep alive time to send pingreq to broker when the connection is idle
    keep_alive_ms: u32,
    /// clean (or) persistent session
    clean_session: bool,
    /// client identifier
    client_id: String,
    /// certificate authority certificate
    ca: Option<&'a [u8]>,
    /// tls client_authentication
    client_auth: Option<(Certificate<'a>, PrivateKey<'a>, Option<Password<'a>>)>,
    /// alpn settings
    // alpn: Option<Vec<Vec<u8>>>,
    /// username and password
    credentials: Option<(String, String)>,
    /// maximum packet size
    max_packet_size: usize,
    /// Minimum delay time between consecutive outgoing packets
    // throttle: Duration,
    /// maximum number of outgoing inflight messages
    inflight: usize,
    /// Last will that will be issued on unexpected disconnect
    last_will: Option<LastWill>,
}

impl<'a> MqttOptions<'a> {
    /// New mqtt options
    pub fn new<S: Into<String>>(id: S, broker: Broker, port: u16) -> MqttOptions<'a> {
        let id = id.into();
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
            max_packet_size: 512,
            // throttle: Duration::from_micros(0),
            inflight: 3,
            last_will: None,
        }
    }

    /// Broker address
    pub fn broker(&self) -> (Broker, u16) {
        (self.broker_addr.clone(), self.port)
    }

    pub fn set_last_will(self, will: LastWill) -> Self {
        Self {
            last_will: Some(will),
            ..self
        }
    }

    pub fn last_will(&self) -> Option<LastWill> {
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
    pub fn client_id(&self) -> String {
        self.client_id.clone()
    }

    /// Set packet size limit (in Kilo Bytes)
    pub fn set_max_packet_size(self, max_packet_size: usize) -> Self {
        Self {
            max_packet_size,
            ..self
        }
    }

    /// Maximum packet size
    pub fn max_packet_size(&self) -> usize {
        self.max_packet_size
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
    pub fn set_credentials<S: Into<String>>(self, username: S, password: S) -> Self {
        Self {
            credentials: Some((username.into(), password.into())),
            ..self
        }
    }

    /// Security options
    pub fn credentials(&self) -> Option<(String, String)> {
        self.credentials.clone()
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

    #[test]
    #[should_panic]
    fn client_id_startswith_space() {
        let _mqtt_opts = MqttOptions::new(" client_a", Ipv4Addr::new(127, 0, 0, 1).into(), 1883)
            .set_clean_session(true);
    }

    #[test]
    #[should_panic]
    fn no_client_id() {
        let _mqtt_opts =
            MqttOptions::new("", Ipv4Addr::localhost().into(), 1883).set_clean_session(true);
    }
}
