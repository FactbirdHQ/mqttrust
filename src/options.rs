use alloc::{string::String, vec::Vec};
use mqttrs::LastWill;
use no_std_net::{Ipv4Addr, SocketAddrV4};

/// Options to configure the behaviour of mqtt connection
#[derive(Clone, Debug)]
pub struct MqttOptions {
    /// broker address that you want to connect to
    broker_addr: Ipv4Addr,
    /// broker port
    port: u16,
    /// keep alive time to send pingreq to broker when the connection is idle
    keep_alive_ms: u32,
    /// clean (or) persistent session
    clean_session: bool,
    /// client identifier
    client_id: String,
    /// connection method
    ca: Option<Vec<u8>>,
    /// tls client_authentication
    client_auth: Option<(Vec<u8>, Vec<u8>)>,
    /// alpn settings
    alpn: Option<Vec<Vec<u8>>>,
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

impl MqttOptions {
    /// New mqtt options
    pub fn new<S: Into<String>, T: Into<Ipv4Addr>>(id: S, host: T, port: u16) -> MqttOptions {
        let id = id.into();
        if id.starts_with(' ') || id.is_empty() {
            panic!("Invalid client id")
        }

        MqttOptions {
            broker_addr: host.into(),
            port,
            keep_alive_ms: 60_000,
            clean_session: true,
            client_id: id,
            ca: None,
            client_auth: None,
            alpn: None,
            credentials: None,
            max_packet_size: 8 * 1024,
            // throttle: Duration::from_micros(0),
            inflight: 100,
            last_will: None,
        }
    }

    /// Broker address
    pub fn broker_address(&self) -> SocketAddrV4 {
        SocketAddrV4::new(self.broker_addr, self.port)
    }

    pub fn set_last_will(&mut self, will: LastWill) -> &mut Self {
        self.last_will = Some(will);
        self
    }

    pub fn last_will(&mut self) -> Option<LastWill> {
        self.last_will.clone()
    }

    pub fn set_ca(&mut self, ca: Vec<u8>) -> &mut Self {
        self.ca = Some(ca);
        self
    }

    pub fn ca(&self) -> Option<Vec<u8>> {
        self.ca.clone()
    }

    pub fn set_client_auth(&mut self, cert: Vec<u8>, key: Vec<u8>) -> &mut Self {
        self.client_auth = Some((cert, key));
        self
    }

    pub fn client_auth(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        self.client_auth.clone()
    }

    pub fn set_alpn(&mut self, alpn: Vec<Vec<u8>>) -> &mut Self {
        self.alpn = Some(alpn);
        self
    }

    pub fn alpn(&self) -> Option<Vec<Vec<u8>>> {
        self.alpn.clone()
    }

    /// Set number of seconds after which client should ping the broker
    /// if there is no other data exchange
    pub fn set_keep_alive(&mut self, secs: u16) -> &mut Self {
        if secs < 5 {
            panic!("Keep alives should be >= 5  secs");
        }

        self.keep_alive_ms = secs as u32 * 1000;
        self
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
    pub fn set_max_packet_size(&mut self, sz: usize) -> &mut Self {
        self.max_packet_size = sz * 1024;
        self
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
    pub fn set_clean_session(&mut self, clean_session: bool) -> &mut Self {
        self.clean_session = clean_session;
        self
    }

    /// Clean session
    pub fn clean_session(&self) -> bool {
        self.clean_session
    }

    /// Username and password
    pub fn set_credentials<S: Into<String>>(&mut self, username: S, password: S) -> &mut Self {
        self.credentials = Some((username.into(), password.into()));
        self
    }

    /// Security options
    pub fn credentials(&self) -> Option<(String, String)> {
        self.credentials.clone()
    }

    // /// Enables throttling and sets outoing message rate to the specified 'rate'
    // pub fn set_throttle(&mut self, duration: Duration) -> &mut Self {
    //     self.throttle = duration;
    //     self
    // }

    // /// Outgoing message rate
    // pub fn throttle(&self) -> Duration {
    //     self.throttle
    // }

    /// Set number of concurrent in flight messages
    pub fn set_inflight(&mut self, inflight: usize) -> &mut Self {
        if inflight == 0 {
            panic!("zero in flight is not allowed")
        }

        self.inflight = inflight;
        self
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
        let _mqtt_opts = MqttOptions::new(" client_a", Ipv4Addr::new(127, 0, 0, 1), 1883)
            .set_clean_session(true);
    }

    #[test]
    #[should_panic]
    fn no_client_id() {
        let _mqtt_opts = MqttOptions::new("", Ipv4Addr::localhost(), 1883).set_clean_session(true);
    }
}
