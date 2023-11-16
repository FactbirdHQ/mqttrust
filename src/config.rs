use core::str::FromStr;

use crate::error::ProtocolError;
use embassy_time::Duration;
use heapless::String;

#[derive(Debug)]
pub struct Config<Broker: crate::Broker> {
    pub(crate) broker: Broker,
    // pub(crate) will: Option<SerializedWill<'a>>,
    pub(crate) client_id: String<64>,
    pub(crate) keepalive_interval: Duration,
    pub(crate) downgrade_qos: bool,
    // pub(crate) auth: Option<Auth<'a>>,
}

impl<Broker: crate::Broker> Config<Broker> {
    /// Construct configuration for the MQTT client.
    pub fn new(client_id: &str, broker: Broker) -> Self {
        Self {
            broker,
            client_id: String::from_str(client_id).unwrap(),
            // auth: None,
            keepalive_interval: Duration::from_secs(59),
            downgrade_qos: false,
            // will: None,
        }
    }

    // /// Specify the authentication message used by the server.
    // ///
    // /// # Args
    // /// * `user_name` - The user name
    // /// * `password` - The password
    // #[cfg(feature = "unsecure")]
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
            String::try_from(id).map_err(|_| ProtocolError::ProvidedClientIdTooLong)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::IpBroker;

    #[test]
    pub fn basic_config() {
        let localhost: embedded_nal_async::IpAddr = "127.0.0.1".parse().unwrap();
        let config: Config<IpBroker> = Config::new("client_id", (localhost, 1883).into());
    }
}
