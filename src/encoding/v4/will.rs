use crate::encoding::QoS;


/// Message that the server should publish when the client disconnects.
///
/// Sent by the client in the [Connect] packet. [MQTT 3.1.3.3].
///
/// [Connect]: struct.Connect.html
/// [MQTT 3.1.3.3]: http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718031
#[derive(Debug, Clone, PartialEq)]
pub struct LastWill<'a> {
    pub topic: &'a str,
    pub message: &'a [u8],
    pub qos: QoS,
    pub retain: bool,
}
