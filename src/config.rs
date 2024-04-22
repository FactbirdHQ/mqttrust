use core::str::FromStr;

use crate::error::ProtocolError;
use embassy_time::Duration;

pub type BackoffAlgo = fn(u8) -> Duration;

#[derive(Debug)]
pub struct Config<Broker> {
    pub broker: Broker,
    // pub will: Option<SerializedWill<'a>>,
    pub client_id: heapless::String<64>,
    pub keepalive_interval: Duration,
    pub connect_timeout: Duration,
    pub downgrade_qos: bool,
    pub backoff_algo: BackoffAlgo,
    // pub auth: Option<Auth<'a>>,
}

impl<Broker> Config<Broker> {
    /// Construct configuration for the MQTT client.
    pub fn new(client_id: &str, broker: Broker) -> Self {
        Self {
            broker,
            client_id: heapless::String::from_str(client_id).unwrap(),
            // auth: None,
            keepalive_interval: Duration::from_secs(59),
            connect_timeout: Duration::from_secs(50),
            downgrade_qos: false,
            backoff_algo: |attempt| {
                let base_time_ms: u32 = 500;
                let backoff = base_time_ms.saturating_mul(u32::pow(2, attempt as u32));
                core::cmp::min(
                    Duration::from_secs(3 * 60),
                    Duration::from_millis(backoff.into()),
                )
            },
            // will: None,
        }
    }

    // /// Specify the authentication message used by the server.
    // ///
    // /// # Args
    // /// * `user_name` - The user name
    // /// * `password` - The password
    // pub fn set_auth(mut self, user_name: &str, password: &str) -> Result<Self, ProtocolError> {
    //     if self.auth.is_some() {
    //         return Err(ProtocolError::AuthAlreadySpecified);
    //     }

    //     let (username_bytes, tail) = self.buffer.split_at_mut(user_name.as_bytes().len());
    //     username_bytes.copy_from_slice(user_name.as_bytes());
    //     self.buffer = tail;

    //     let (password_bytes, tail) = self.buffer.split_at_mut(password.as_bytes().len());
    //     password_bytes.copy_from_slice(password.as_bytes());
    //     self.buffer = tail;

    //     self.auth.replace(Auth {
    //         // Note(unwrap): We are directly copying `str` types to these buffers, so we know they
    //         // are valid utf8.
    //         user_name: core::str::from_utf8(username_bytes).unwrap(),
    //         password: core::str::from_utf8(password_bytes).unwrap(),
    //     });
    //     Ok(self)
    // }

    /// Specify a known client ID to use. If not assigned, the broker will auto assign an ID.
    pub fn client_id(mut self, id: &str) -> Result<Self, ProtocolError> {
        self.client_id =
            heapless::String::try_from(id).map_err(|_| ProtocolError::ProvidedClientIdTooLong)?;
        Ok(self)
    }

    /// Configure the MQTT keep-alive interval.
    ///
    /// # Note
    /// The broker may override the requested keep-alive interval. Any value requested by the
    /// broker will be used instead.
    ///
    /// # Args
    /// * `interval` - The keep-alive interval in seconds. A ping will be transmitted if no other
    /// messages are sent within 50% of the keep-alive interval.
    pub fn keepalive_interval(mut self, duration: Duration) -> Self {
        self.keepalive_interval = duration;
        self
    }

    /// Specify if publication [QoS] should be automatically downgraded to the maximum supported by
    /// the server if they exceed the server [QoS] maximum.
    pub fn autodowngrade_qos(mut self) -> Self {
        self.downgrade_qos = true;
        self
    }

    // /// Specify the Will message to be sent if the client disconnects.
    // ///
    // /// # Args
    // /// * `will` - The will to use.
    // pub fn will(mut self, will: Will<'_>) -> Result<Self, ProtocolError> {
    //     if self.will.is_some() {
    //         return Err(ProtocolError::WillAlreadySpecified);
    //     }
    //     let will_len = will.serialized_len();
    //     let (head, tail) = self.buffer.split_at_mut(will_len);
    //     self.buffer = tail;
    //     self.will = Some(will.serialize(head)?);

    //     Ok(self)
    // }

    // /// Configure the TLS options on the socket.
    // ///
    // /// # Note
    // /// If a broker is already set, that returns `Some(hostname)` on
    // /// `Broker::get_hostname()`, then the server_name in the tls config will be
    // /// automatically set to that hostname.
    // ///
    // /// # Args
    // /// * `tls_config` - The TLS config to be passed on to `embedded-tls` when
    // ///   doing TLS handshake during TCP connect.
    // #[cfg(feature = "embedded-tls")]
    // pub fn tls_config(mut self, mut tls_config: embedded_tls::TlsConfig<'a>) -> Config<'a, Broker> {
    //     tls_config.server_name = self.broker.get_hostname();
    //     self.tls_config = tls_config;

    //     self
    // }
}
