use crate::{crate_config::MAX_CLIENT_ID_LEN, QoS};
use bon::Builder;
use embassy_time::Duration;

/// Type alias for a backoff algorithm function, which takes the number of attempts and returns a `Duration`.
pub type BackoffAlgo = fn(u8) -> Duration;

/// Default backoff algorithm for reconnection attempts.
///
/// This algorithm uses exponential backoff, starting with a base time of 500 milliseconds and doubling
/// the wait time for each subsequent attempt, up to a maximum of 3 minutes.
const DEFAULT_BACKOFF: BackoffAlgo = |attempt| {
    let base_time_ms: u32 = 500;
    let backoff = base_time_ms.saturating_mul(u32::pow(2, attempt as u32));
    core::cmp::min(
        Duration::from_secs(3 * 60),
        Duration::from_millis(backoff.into()),
    )
};

/// Maximum Quality of Service (QoS) level supported by the client.
///
/// This is determined by whether the `qos2` feature is enabled. If enabled, the maximum QoS is `ExactlyOnce`.
/// Otherwise, it is `AtLeastOnce`.
#[cfg(feature = "qos2")]
const MAX_QOS: QoS = QoS::ExactlyOnce;
#[cfg(not(feature = "qos2"))]
const MAX_QOS: QoS = QoS::AtLeastOnce;

/// Configuration settings for the MQTT client.
///
/// This struct contains various settings that control the behavior of the MQTT client, such as the client ID,
/// keepalive interval, connection timeout, backoff algorithm for reconnection attempts, and maximum QoS level.
#[derive(Debug, Builder)]
pub struct Config {
    /// The client ID to use when connecting to the MQTT broker.
    ///
    /// This must be a unique identifier for the client within the broker.
    pub(crate) client_id: heapless::String<MAX_CLIENT_ID_LEN>,

    /// The keepalive interval for the MQTT connection.
    ///
    /// This is the maximum time interval that is permitted to elapse between the point at which the client finishes
    /// transmitting one control packet and the point it starts transmitting the next. The default is 59 seconds.
    #[builder(default = Duration::from_secs(59))]
    pub(crate) keepalive_interval: Duration,

    /// The timeout duration for establishing a connection to the MQTT broker.
    ///
    /// If the client is unable to establish a connection within this time, it will abort the attempt. The default is 50 seconds.
    #[builder(default = Duration::from_secs(50))]
    pub(crate) connect_timeout: Duration,

    /// The backoff algorithm to use for reconnection attempts.
    ///
    /// This function takes the number of attempts and returns the duration to wait before the next attempt.
    /// The default is an exponential backoff algorithm.
    #[builder(default = DEFAULT_BACKOFF)]
    pub(crate) backoff_algo: BackoffAlgo,

    /// The maximum Quality of Service (QoS) level supported by the client.
    ///
    /// This determines the highest QoS level that the client will use for publishing and subscribing to topics.
    /// The default is determined by whether the `qos2` feature is enabled.
    #[builder(default = MAX_QOS)]
    pub(crate) max_qos: QoS,
    // pub(crate) will: Option<SerializedWill<'a>>,
    // #[builder(default = false)]
    // pub(crate) downgrade_qos: bool,
    // pub(crate) auth: Option<Auth<'a>>,
}
