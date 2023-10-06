use core::task::Context;

use embassy_sync::waitqueue::WakerRegistration;
use embassy_time::Instant;
use heapless::{FnvIndexMap, IndexMap};

use crate::encoding::v4::Pid;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ConnectionState {
    Disconnected,
    Connected,
}

pub struct Shared {
    /// Packet id of the last outgoing packet
    last_pid: Pid,

    conn_state: ConnectionState,

    tx_waker: WakerRegistration,

    /// Status of last ping
    await_pingresp: bool,

    /// Outgoing QoS 1, 2 publishes which aren't acked yet
    pub(crate) outgoing_pub: FnvIndexMap<u16, Inflight<1536>, 2>,

    /// Packet ids of released QoS 2 publishes
    #[cfg(feature = "qos2")]
    outgoing_rel: heapless::FnvIndexSet<u16, 2>,

    /// Packet ids on incoming QoS 2 publishes
    #[cfg(feature = "qos2")]
    incoming_pub: heapless::FnvIndexSet<u16, 2>,

    last_ping: Option<Instant>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            last_pid: Pid::new(),

            conn_state: ConnectionState::Disconnected,
            tx_waker: WakerRegistration::new(),

            await_pingresp: false,
            last_ping: None,

            outgoing_pub: IndexMap::new(),

            #[cfg(feature = "qos2")]
            outgoing_rel: heapless::IndexSet::new(),
            #[cfg(feature = "qos2")]
            incoming_pub: heapless::IndexSet::new(),
        }
    }

    pub fn next_pid(&mut self) -> Pid {
        self.last_pid = self.last_pid + 1;
        self.last_pid
    }

    pub fn register_tx_waker(&mut self, cx: &Context<'_>) {
        self.tx_waker.register(cx.waker())
    }
}

/// Client publication message data.
#[derive(Debug)]
pub(crate) struct Inflight<const L: usize> {
    /// A publish of non-zero QoS.
    publish: heapless::Vec<u8, L>,
    /// A timestmap used for retry and expiry.
    last_touch: Instant,
}

impl<const L: usize> Inflight<L> {
    pub(crate) fn new(publish: &[u8]) -> Self {
        // assert!(
        //     !matches!(
        //         decoder::Header::new(publish[0]).unwrap().qos,
        //         crate::QoS::AtMostOnce
        //     ),
        //     "Only non-zero QoSs are allowed."
        // );
        Self {
            publish: heapless::Vec::from_slice(publish).unwrap(),
            last_touch: Instant::now(),
        }
    }

    pub(crate) fn update_last_touch(&mut self) {
        self.last_touch = Instant::now();
    }
}

impl<const L: usize> Inflight<L> {
    // pub(crate) fn packet<'b>(&'b mut self, pid: u16) -> Result<&'b [u8], StateError> {
    //     let pid = pid.try_into().map_err(|_| StateError::PayloadEncoding)?;
    //     let mut packet = SerializedPacket(self.publish.as_mut());
    //     packet.set_pid(pid)?;
    //     Ok(packet.to_inner())
    // }
}
