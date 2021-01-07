use mqttrs::LastWill;

/// Options to configure the behaviour of mqtt connection
///
/// **Lifetimes**:
/// - 'a: The lifetime of option fields, not referenced in any MQTT packets at any point
/// - 'b: The lifetime of the packet fields, backed by a slice buffer
#[derive(Clone, Debug)]
pub struct MqttOptions<'a> {
    /// keep alive time to send pingreq to broker when the connection is idle
    keep_alive_ms: u32,
    /// clean (or) persistent session
    clean_session: bool,
    /// client identifier
    client_id: &'a str,
    /// username and password
    credentials: Option<(&'a str, &'a [u8])>,
    /// Minimum delay time between consecutive outgoing packets
    /// maximum number of outgoing inflight messages
    inflight: usize,
    /// Last will that will be issued on unexpected disconnect
    last_will: Option<LastWill<'a>>,
}

impl<'a> MqttOptions<'a> {
    /// New mqtt options
    pub fn new(id: &'a str) -> MqttOptions<'a> {
        if id.starts_with(' ') || id.is_empty() {
            panic!("Invalid client id")
        }

        MqttOptions {
            keep_alive_ms: 60_000,
            clean_session: true,
            client_id: id,
            credentials: None,
            inflight: 3,
            last_will: None,
        }
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
    use super::MqttOptions;
    use mqttrs::LastWill;

    #[test]
    #[should_panic]
    fn client_id_starts_with_space() {
        let _mqtt_opts = MqttOptions::new(" client_a").set_clean_session(true);
    }

    #[test]
    #[should_panic]
    fn no_client_id() {
        let _mqtt_opts = MqttOptions::new("").set_clean_session(true);
    }

    #[test]
    fn client_id() {
        let opts = MqttOptions::new("client_a");
        assert_eq!(opts.client_id(), "client_a");
    }

    #[test]
    fn inflight() {
        let opts = MqttOptions::new("client_a");
        assert_eq!(opts.inflight, 3);
        assert_eq!(opts.set_inflight(5).inflight(), 5);
    }

    #[test]
    #[should_panic]
    fn zero_inflight() {
        let opts = MqttOptions::new("client_a");
        assert_eq!(opts.inflight, 3);
        assert_eq!(opts.set_inflight(0).inflight(), 5);
    }

    #[test]
    fn keep_alive_ms() {
        let opts = MqttOptions::new("client_a");
        assert_eq!(opts.keep_alive_ms, 60_000);
        assert_eq!(opts.set_keep_alive(120).keep_alive_ms(), 120_000);
    }

    #[test]
    #[should_panic]
    fn keep_alive_panic() {
        let opts = MqttOptions::new("client_a");
        assert_eq!(opts.keep_alive_ms, 60_000);
        assert_eq!(opts.set_keep_alive(4).keep_alive_ms(), 120_000);
    }

    #[test]
    fn last_will() {
        let opts = MqttOptions::new("client_a");
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
        let opts = MqttOptions::new("client_a");
        assert_eq!(opts.clean_session, true);
        assert_eq!(opts.set_clean_session(false).clean_session(), false);
    }

    #[test]
    fn credentials() {
        let opts = MqttOptions::new("client_a");
        assert_eq!(opts.credentials, None);
        assert_eq!(opts.credentials(), (None, None));
        assert_eq!(
            opts.set_credentials("some_user", &[]).credentials(),
            (Some("some_user"), Some(&b""[..]))
        );
    }
}
