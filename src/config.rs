use crate::{crate_config::MAX_CLIENT_ID_LEN, QoS};
use bon::Builder;
use embassy_time::Duration;

pub type BackoffAlgo = fn(u8) -> Duration;

const DEFAULT_BACKOFF: BackoffAlgo = |attempt| {
    let base_time_ms: u32 = 500;
    let backoff = base_time_ms.saturating_mul(u32::pow(2, attempt as u32));
    core::cmp::min(
        Duration::from_secs(3 * 60),
        Duration::from_millis(backoff.into()),
    )
};

#[cfg(feature = "qos2")]
const MAX_QOS: QoS = QoS::ExactlyOnce;
#[cfg(not(feature = "qos2"))]
const MAX_QOS: QoS = QoS::AtLeastOnce;

#[derive(Debug, Builder)]
pub struct Config {
    // pub(crate) will: Option<SerializedWill<'a>>,
    pub(crate) client_id: heapless::String<MAX_CLIENT_ID_LEN>,

    #[builder(default = Duration::from_secs(59))]
    pub(crate) keepalive_interval: Duration,

    #[builder(default = Duration::from_secs(50))]
    pub(crate) connect_timeout: Duration,

    // #[builder(default = false)]
    // pub(crate) downgrade_qos: bool,
    #[builder(default = DEFAULT_BACKOFF)]
    pub(crate) backoff_algo: BackoffAlgo,
    // TODO: Check `max_qos` in packet handling
    #[builder(default = MAX_QOS)]
    pub(crate) max_qos: QoS,
    // pub(crate) auth: Option<Auth<'a>>,
}
