use core::task::Context;

use embassy_sync::waitqueue::MultiWakerRegistration;
use embassy_time::Instant;
use heapless::{FnvIndexMap, IndexMap};

use crate::encoding::Pid;

const MAX_INFLIGHT: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum AckStatus {
    AwaitingSubAck,
    AwaitingUnsubAck,
    Acked(heapless::Vec<u8, 10>),
}

pub struct Shared<const SUBS: usize> {
    /// Packet id of the last outgoing packet
    last_pid: Pid,

    tx_waker: MultiWakerRegistration<MAX_INFLIGHT>,

    /// Outgoing QoS 1, 2 publishes which aren't acked yet
    pub(crate) inflight_pub: FnvIndexMap<u16, Inflight<4>, MAX_INFLIGHT>,
    pub(crate) ack_status: FnvIndexMap<u16, AckStatus, SUBS>,
    pub(crate) outgoing_pid: heapless::FnvIndexSet<u16, MAX_INFLIGHT>,

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

            tx_waker: MultiWakerRegistration::new(),

            inflight_pub: IndexMap::new(),
            ack_status: IndexMap::new(),
            outgoing_pid: heapless::IndexSet::new(),

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
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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
            publish: heapless::Vec::from_slice(&[]).unwrap(),
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
