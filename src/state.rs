use core::task::Context;

use embassy_sync::waitqueue::MultiWakerRegistration;
use embassy_time::Instant;
use heapless::{FnvIndexMap, IndexMap};

use crate::encoding::Pid;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ConnectionState {
    Disconnected,
    Connected,
}

const MAX_INFLIGHT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingAck {
    Subscribe(u16),
    Unsubscribe(u16),
}

impl hash32::Hash for PendingAck {
    fn hash<H: hash32::Hasher>(&self, h: &mut H) -> () {
        match self {
            PendingAck::Subscribe(pid) => {
                h.write(&pid.to_le_bytes());
                h.write(&[0]);
            }
            PendingAck::Unsubscribe(pid) => {
                h.write(&pid.to_le_bytes());
                h.write(&[1]);
            }
        }
        h.finish();
    }
}

pub struct Shared<const SUBS: usize> {
    /// Packet id of the last outgoing packet
    last_pid: Pid,

    conn_state: ConnectionState,

    tx_waker: MultiWakerRegistration<MAX_INFLIGHT>,

    /// Outgoing QoS 1, 2 publishes which aren't acked yet
    pub(crate) outgoing_pub: FnvIndexMap<u16, Inflight<1536>, MAX_INFLIGHT>,
    pub(crate) pending_ack: heapless::FnvIndexSet<PendingAck, SUBS>,

    /// Packet ids of released QoS 2 publishes
    #[cfg(feature = "qos2")]
    pub(crate) outgoing_rel: heapless::FnvIndexSet<u16, MAX_INFLIGHT>,

    /// Packet ids on incoming QoS 2 publishes
    #[cfg(feature = "qos2")]
    pub(crate) incoming_pub: heapless::FnvIndexSet<u16, MAX_INFLIGHT>,
}

impl<const SUBS: usize> Shared<SUBS> {
    pub fn new() -> Self {
        Self {
            last_pid: Pid::new(),

            conn_state: ConnectionState::Disconnected,
            tx_waker: MultiWakerRegistration::new(),

            outgoing_pub: IndexMap::new(),
            pending_ack: heapless::IndexSet::new(),

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

    pub fn wake_tx(&mut self) {
        self.tx_waker.wake()
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
