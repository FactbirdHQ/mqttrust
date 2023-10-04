//! Implementation of anything directly publisher related

use core::future::Future;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::sync::atomic::Ordering;
use core::task::{Context, Poll};

use super::{Error, PubSubChannel, PubSubState};
use embassy_sync::blocking_mutex::raw::RawMutex;

/// A publisher to a channel
pub struct Publisher<'a, M: RawMutex, const CAP: usize, const SUBS: usize> {
    /// The channel we are a publisher for
    channel: &'a PubSubChannel<M, CAP, SUBS>,
}

impl<'a, M: RawMutex, const CAP: usize, const SUBS: usize> Publisher<'a, M, CAP, SUBS> {
    pub(super) fn new(channel: &'a PubSubChannel<M, CAP, SUBS>) -> Self {
        Self { channel }
    }

    pub fn grant_exact(&mut self, sz: usize) -> Result<GrantW<'a, CAP, SUBS>, Error> {
        self.channel.grant_exact_with_context(sz, None)
    }

    // pub fn grant_max_remaining(&mut self, mut sz: usize) -> Result<GrantW<'a, CAP, SUBS>, Error> {
    //     self.channel.grant_max_remaining_with_context(sz, None)
    // }

    // pub fn grant_exact_async(&mut self, sz: usize) -> GrantExactFuture<'_, CAP, SUBS> {
    //     GrantExactFuture { pbs: self.channel, sz }
    // }

    // /// Async version of [Self::grant_max_remaining].
    // /// Will wait for the buffer to at least 1 byte available, as soon as it does, return the grant.
    // pub fn grant_max_remaining_async(
    //     &'_ mut self,
    //     sz: usize,
    // ) -> GrantMaxRemainingFuture<'a, '_> {
    //     GrantMaxRemainingFuture { prod: self, sz }
    // }
}

impl<'a, M: RawMutex, const CAP: usize, const SUBS: usize> Drop for Publisher<'a, M, CAP, SUBS> {
    fn drop(&mut self) {
        self.channel.unregister_publisher()
    }
}

/// A structure representing a contiguous region of memory that
/// may be written to, and potentially "committed" to the queue.
///
/// NOTE: If the grant is dropped without explicitly commiting
/// the contents, or by setting a the number of bytes to
/// automatically be committed with `to_commit()`, then no bytes
/// will be comitted for writing.
///
/// If the `thumbv6` feature is selected, dropping the grant
/// without committing it takes a short critical section,
pub struct GrantW<'a, const CAP: usize, const SUBS: usize> {
    pub(crate) buf: &'a mut [u8],
    pub(crate) pbs: &'a PubSubState<CAP, SUBS>,
    pub(crate) to_commit: usize,
}

impl<'a, const CAP: usize, const SUBS: usize> GrantW<'a, CAP, SUBS> {
    /// Finalizes a writable grant given by `grant()` or `grant_max()`.
    /// This makes the data available to be read via `read()`. This consumes
    /// the grant.
    ///
    /// If `used` is larger than the given grant, the maximum amount will
    /// be commited
    ///
    /// NOTE:  If the `thumbv6` feature is selected, this function takes a short critical
    /// section while committing.
    pub fn commit(mut self, used: usize) {
        self.commit_inner(used);
        core::mem::forget(self);
    }

    /// Obtain access to the inner buffer for writing
    ///
    /// ```rust
    /// # // bbqueue test shim!
    /// # fn bbqtest() {
    /// use bbqueue::{BBQueue, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let buffer: BBQueue<StaticBufferProvider<6>> = BBQueue::new_static();
    /// let (mut prod, mut cons) = buffer.try_split().unwrap();
    ///
    /// // Successfully obtain and commit a grant of four bytes
    /// let mut grant = prod.grant_max_remaining(4).unwrap();
    /// grant.buf().copy_from_slice(&[1, 2, 3, 4]);
    /// grant.commit(4);
    /// # // bbqueue test shim!
    /// # }
    /// #
    /// # fn main() {
    /// # #[cfg(not(feature = "thumbv6"))]
    /// # bbqtest();
    /// # }
    /// ```
    pub fn buf(&mut self) -> &mut [u8] {
        self.buf
    }

    /// Sometimes, it's not possible for the lifetimes to check out. For example,
    /// if you need to hand this buffer to a function that expects to receive a
    /// `&'static mut [u8]`, it is not possible for the inner reference to outlive the
    /// grant itself.
    ///
    /// You MUST guarantee that in no cases, the reference that is returned here outlives
    /// the grant itself. Once the grant has been released, referencing the data contained
    /// WILL cause undefined behavior.
    ///
    /// Additionally, you must ensure that a separate reference to this data is not created
    /// to this data, e.g. using `DerefMut` or the `buf()` method of this grant.
    pub unsafe fn as_static_mut_buf(&mut self) -> &'static mut [u8] {
        core::mem::transmute::<&mut [u8], &'static mut [u8]>(self.buf)
    }

    #[inline(always)]
    pub(crate) fn commit_inner(&mut self, used: usize) {
        // If there is no grant in progress, return early. This
        // generally means we are dropping the grant within a
        // wrapper structure
        if !self.pbs.write_in_progress.load(Ordering::Acquire) {
            return;
        }

        // Writer component. Must never write to READ,
        // be careful writing to LAST

        // Saturate the grant commit
        let len = self.buf.len();
        let used = core::cmp::min(len, used);

        let write = self.pbs.write.load(Ordering::Acquire);
        self.pbs.reserve.fetch_sub(len - used, Ordering::AcqRel);

        let max = len;
        let last = self.pbs.last.load(Ordering::Acquire);
        let new_write = self.pbs.reserve.load(Ordering::Acquire);

        if (new_write < write) && (write != max) {
            // We have already wrappedut we are skipping some bytes at the end of the ring.
            // Mark `last` where the write pointer used to be to hold the line here
            self.pbs.last.store(write, Ordering::Release);
        } else if new_write > last {
            // We're about to pass the last pointer, which was previously the artificial
            // end of the ring. Now that we've passed it, we can "unlock" the section
            // that was previously skipped.
            //
            // Since new_write is strictly larger than last, it is safe to move this as
            // the other thread will still be halted by the (about to be updated) write
            // value
            self.pbs.last.store(max, Ordering::Release);
        }
        // else: If new_write == last, either:
        // * last == max, so no need to write, OR
        // * If we write in the end chunk again, we'll update last to max next time
        // * If we write to the start chunk in a wrap, we'll update last when we
        //     move write backwards

        // Write must be updated AFTER last, otherwise read could think it was
        // time to invert early!
        self.pbs.write.store(new_write, Ordering::Release);

        // Allow subsequent grants
        self.pbs.write_in_progress.store(false, Ordering::Release);

        self.pbs.subscriber_wakers.wake();

        // unsafe {
        //     self.bbq.as_mut().read_waker.wake();
        // };
    }

    /// Configures the amount of bytes to be commited on drop.
    pub fn to_commit(&mut self, amt: usize) {
        self.to_commit = self.buf.len().min(amt);
    }
}

impl<'a, const CAP: usize, const SUBS: usize> Drop for GrantW<'a, CAP, SUBS> {
    fn drop(&mut self) {
        self.commit_inner(self.to_commit)
    }
}

impl<'a, const CAP: usize, const SUBS: usize> Deref for GrantW<'a, CAP, SUBS> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buf
    }
}

impl<'a, const CAP: usize, const SUBS: usize> DerefMut for GrantW<'a, CAP, SUBS> {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.buf
    }
}

// /// Future returned [Producer::grant_exact_async]
// pub struct GrantExactFuture<'a, const CAP: usize, const SUBS: usize> {
//     pub(crate) pbs: &'a PubSubState<CAP, SUBS>,
//     sz: usize,
// }

// impl<'a, const CAP: usize, const SUBS: usize> Future
//     for GrantExactFuture<'a, CAP, SUBS>
// {
//     type Output = Result<GrantW<'a, CAP, SUBS>, Error>;

//     fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//         match self.pbs.try_grant_exact(self.sz) {
//             // We did it, we are ready
//             Ok(grant) => Ok(grant),
//             // The queue is full, so we need to reregister our waker and go to sleep
//             Err(message) => {
//                 if let Some(cx) = cx {
//                     self.pbs.publisher_waker.register(cx.waker());
//                 }
//                 Err(message)
//             }
//         }
//     }
// }

// /// Future returned [Producer::grant_max_remaining_async]
// pub struct GrantMaxRemainingFuture<'a, 'b> {
//     prod: &'b mut Producer<'a>,
//     sz: usize,
// }

// impl<'a, 'b, const CAP: usize, const SUBS: usize> Future for GrantMaxRemainingFuture<'a, 'b> {
//     type Output = Result<GrantW<'a, CAP, SUBS>, Error>;

//     fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//         let sz = self.sz;

//         match self.prod.grant_max_remaining(sz) {
//             Ok(grant) => Poll::Ready(Ok(grant)),
//             Err(e) => match e {
//                 Error::GrantInProgress | Error::InsufficientSize => {
//                     unsafe { self.prod.bbq.as_mut().write_waker.set(cx.waker()) };
//                     Poll::Pending
//                 }
//                 _ => Poll::Ready(Err(e)),
//             },
//         }
//     }
// }
