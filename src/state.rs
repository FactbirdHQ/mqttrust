use core::task::Context;

use embassy_sync::waitqueue::{MultiWakerRegistration, WakerRegistration};
use heapless::{FnvIndexMap, IndexMap};

use crate::crate_config::{MAX_INFLIGHT, MAX_SUBSCRIBERS, MAX_SUB_TOPICS_PER_MSG};
use crate::encoding::Pid;

#[derive(Debug, PartialEq, Eq, Hash)]
pub(crate) enum AckStatus {
    AwaitingSubAck,
    AwaitingUnsubAck(bool),
    Acked(heapless::Vec<u8, MAX_SUB_TOPICS_PER_MSG>),
}

pub struct Shared {
    /// Packet id of the last outgoing packet
    last_pid: Pid,

    tx_waker: MultiWakerRegistration<MAX_INFLIGHT>,

    pub(crate) connection_waker: WakerRegistration,

    /// Waker for connection state changes (both connect and disconnect)
    pub(crate) connection_state_change_waker: WakerRegistration,

    /// Whether we are currently connected to the broker.
    ///
    /// - `Some(true)` if using an existing session
    /// - `Some(false)` if using an clean session
    /// - `None` if disconnected
    pub(crate) connected: Option<bool>,

    /// Outgoing QoS 1, 2 publishes which aren't acked yet
    pub(crate) inflight_pub: heapless::FnvIndexSet<u16, MAX_INFLIGHT>,
    pub(crate) ack_status: FnvIndexMap<u16, AckStatus, { MAX_SUBSCRIBERS }>,
    pub(crate) outgoing_pid: heapless::FnvIndexSet<u16, MAX_INFLIGHT>,

    /// Packet ids of released QoS 2 publishes
    #[cfg(feature = "qos2")]
    pub(crate) outgoing_rel: heapless::FnvIndexSet<u16, MAX_INFLIGHT>,

    /// Packet ids on incoming QoS 2 publishes
    #[cfg(feature = "qos2")]
    pub(crate) incoming_pub: heapless::FnvIndexSet<u16, MAX_INFLIGHT>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            last_pid: Pid::new(),

            tx_waker: MultiWakerRegistration::new(),

            connection_waker: WakerRegistration::new(),
            connection_state_change_waker: WakerRegistration::new(),
            connected: None,

            inflight_pub: heapless::IndexSet::new(),
            ack_status: IndexMap::new(),
            outgoing_pid: heapless::IndexSet::new(),

            #[cfg(feature = "qos2")]
            outgoing_rel: heapless::IndexSet::new(),
            #[cfg(feature = "qos2")]
            incoming_pub: heapless::IndexSet::new(),
        }
    }

    pub fn reset(&mut self) {
        self.set_connected(None);

        self.inflight_pub.clear();
        self.ack_status.clear();
        self.outgoing_pid.clear();

        #[cfg(feature = "qos2")]
        self.outgoing_rel.clear();

        #[cfg(feature = "qos2")]
        self.incoming_pub.clear();
    }

    pub fn next_pid(&mut self) -> Pid {
        self.last_pid = self.last_pid + 1;
        self.last_pid
    }

    pub fn register_tx_waker(&mut self, cx: &Context<'_>) {
        self.tx_waker.register(cx.waker())
    }

    pub fn wake_tx(&mut self) {
        self.tx_waker.wake()
    }

    pub fn set_connected(&mut self, connected: Option<bool>) {
        let previous = self.connected;
        self.connected = connected;
        self.connection_waker.wake();

        // Wake state change waker if connection state actually changed
        if previous != connected {
            self.connection_state_change_waker.wake();
        }
    }
}
