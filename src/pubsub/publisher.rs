use core::cell::RefCell;
use core::cmp::min;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::slice::from_raw_parts_mut;
use core::task::{Context, Poll};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use futures::Future;

use super::{state::PubSubState, BufferProvider, Error, Result};

/// `Publisher` is the primary interface for pushing data into a `PubSubState`.
/// There are various methods for obtaining a grant to write to the buffer, with
/// different potential tradeoffs. As all grants are required to be a contiguous
/// range of data, different strategies are sometimes useful when making the decision
/// between maximizing usage of the buffer, and ensuring a given grant is successful.
///
/// As a short summary of currently possible grants:
///
/// * `grant_exact(N)`
///   * User will receive a grant `sz == N` (or receive an error)
///   * This may cause a wraparound if a grant of size N is not available
///       at the end of the ring.
///   * If this grant caused a wraparound, the bytes that were "skipped" at the
///       end of the ring will not be available until the reader reaches them,
///       regardless of whether the grant commited any data or not.
///   * Maximum possible waste due to skipping: `N - 1` bytes
/// * `grant_max_remaining(N)`
///   * User will receive a grant `0 < sz <= N` (or receive an error)
///   * This will only cause a wrap to the beginning of the ring if exactly
///       zero bytes are available at the end of the ring.
///   * Maximum possible waste due to skipping: 0 bytes
///
/// See [this github issue](https://github.com/jamesmunns/PubSubState/issues/38) for a
/// discussion of grant methods that could be added in the future.
pub struct Publisher<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    channel: &'a Mutex<NoopRawMutex, RefCell<PubSubState<B, SUBS>>>,
}

unsafe impl<'a, B: BufferProvider, const SUBS: usize> Send for Publisher<'a, B, SUBS> {}

impl<'a, B, const SUBS: usize> Publisher<'a, B, SUBS>
where
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a Mutex<NoopRawMutex, RefCell<PubSubState<B, SUBS>>>) -> Self {
        Self { channel }
    }

    /// Request a writable, contiguous section of memory of exactly
    /// `sz` bytes. If the buffer size requested is not available,
    /// an error will be returned.
    ///
    /// This method may cause the buffer to wrap around early if the
    /// requested space is not available at the end of the buffer, but
    /// is available at the beginning
    ///
    /// ```rust
    /// # // PubSubState test shim!
    /// # fn channeltest() {
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let buffer: PubSubState<StaticBufferProvider<6>> = PubSubState::new_static();
    /// let (mut prod, cons) = buffer.try_split().unwrap();
    ///
    /// // Successfully obtain and commit a grant of four bytes
    /// let mut grant = prod.grant_exact(4).unwrap();
    /// assert_eq!(grant.buf().len(), 4);
    /// grant.commit(4);
    ///
    /// // Try to obtain a grant of three bytes
    /// assert!(prod.grant_exact(3).is_err());
    /// # // PubSubState test shim!
    /// # }
    /// #
    /// # fn main() {
    /// # #[cfg(not(feature = "thumbv6"))]
    /// # channeltest();
    /// # }
    /// ```
    pub fn grant_exact(&mut self, sz: usize) -> Result<GrantW<'a, B, SUBS>> {
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();
            if inner.write_in_progress {
                return Err(Error::GrantInProgress);
            }
            inner.write_in_progress = true;

            // Writer component. Must never write to `read`,
            // be careful writing to `load`
            let max = inner.capacity();
            let already_inverted = inner.write < inner.read;

            let start = if already_inverted {
                if (inner.write + sz) < inner.read {
                    // Inverted, room is still available
                    inner.write
                } else {
                    // Inverted, no room is available
                    inner.write_in_progress = false;
                    return Err(Error::InsufficientSize);
                }
            } else {
                if inner.write + sz <= max {
                    // Non inverted condition
                    inner.write
                } else {
                    // Not inverted, but need to go inverted

                    // NOTE: We check sz < read, NOT <=, because
                    // write must never == read in an inverted condition, since
                    // we will then not be able to tell if we are inverted or not
                    if sz < inner.read {
                        // Invertible situation
                        0
                    } else {
                        // Not invertible, no space
                        inner.write_in_progress = false;
                        return Err(Error::InsufficientSize);
                    }
                }
            };

            // Safe write, only viewed by this task
            inner.reserve = start + sz;

            // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
            // are all `#[repr(Transparent)]
            let start_of_buf_ptr =
                unsafe { (&mut *inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
            let grant_slice =
                unsafe { from_raw_parts_mut(start_of_buf_ptr.offset(start as isize), sz) };

            Ok(GrantW {
                buf: grant_slice,
                channel: self.channel,
                to_commit: 0,
            })
        })
    }

    /// Request a writable, contiguous section of memory of up to
    /// `sz` bytes. If a buffer of size `sz` is not available without
    /// wrapping, but some space (0 < available < sz) is available without
    /// wrapping, then a grant will be given for the remaining size at the
    /// end of the buffer. If no space is available for writing, an error
    /// will be returned.
    ///
    /// ```
    /// # // PubSubState test shim!
    /// # fn channeltest() {
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let mut buffer: PubSubState<StaticBufferProvider<6>> = PubSubState::new_static();
    /// let (mut prod, mut cons) = buffer.try_split().unwrap();
    ///
    /// // Successfully obtain and commit a grant of four bytes
    /// let mut grant = prod.grant_max_remaining(4).unwrap();
    /// assert_eq!(grant.buf().len(), 4);
    /// grant.commit(4);
    ///
    /// // Release the four initial commited bytes
    /// let mut grant = cons.read().unwrap();
    /// assert_eq!(grant.buf().len(), 4);
    /// grant.release(4);
    ///
    /// // Try to obtain a grant of three bytes, get two bytes
    /// let mut grant = prod.grant_max_remaining(3).unwrap();
    /// assert_eq!(grant.buf().len(), 2);
    /// grant.commit(2);
    /// # // PubSubState test shim!
    /// # }
    /// #
    /// # fn main() {
    /// # #[cfg(not(feature = "thumbv6"))]
    /// # channeltest();
    /// # }
    /// ```
    pub fn grant_max_remaining(&mut self, mut sz: usize) -> Result<GrantW<'a, B, SUBS>> {
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();

            if inner.write_in_progress {
                return Err(Error::GrantInProgress);
            }
            inner.write_in_progress = true;

            // Writer component. Must never write to `read`,
            // be careful writing to `load`
            let max = inner.capacity();

            let already_inverted = inner.write < inner.read;

            let start = if already_inverted {
                // In inverted case, read is always > write
                let remain = inner.read - inner.write - 1;

                if remain != 0 {
                    sz = min(remain, sz);
                    inner.write
                } else {
                    // Inverted, no room is available
                    inner.write_in_progress = false;
                    return Err(Error::InsufficientSize);
                }
            } else {
                if inner.write != max {
                    // Some (or all) room remaining in un-inverted case
                    sz = min(max - inner.write, sz);
                    inner.write
                } else {
                    // Not inverted, but need to go inverted

                    // NOTE: We check read > 1, NOT read >= 1, because
                    // write must never == read in an inverted condition, since
                    // we will then not be able to tell if we are inverted or not
                    if inner.read > 1 {
                        sz = min(inner.read - 1, sz);
                        0
                    } else {
                        // Not invertible, no space
                        inner.write_in_progress = false;
                        return Err(Error::InsufficientSize);
                    }
                }
            };

            // Safe write, only viewed by this task
            inner.reserve = start + sz;

            // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
            // are all `#[repr(Transparent)]
            let start_of_buf_ptr =
                unsafe { (&mut *inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
            let grant_slice =
                unsafe { from_raw_parts_mut(start_of_buf_ptr.offset(start as isize), sz) };

            Ok(GrantW {
                buf: grant_slice,
                channel: self.channel,
                to_commit: 0,
            })
        })
    }

    /// Async version of [Self::grant_exact].
    /// If the buffer can enventually provide a buffer of the requested size, the future
    /// will wait for the buffer to be read so the exact buffer can be requested.
    ///
    /// If it's not possible to request it, an error is returned.
    /// For example, given a buffer
    /// [0|1|2|3|4|5|6|7|8]
    ///              ^
    ///              Write pointer
    /// We cannot request a size of size 7, since we would loop over the read pointer
    /// even if the buffer is empty. In this case, an error is returned
    pub fn grant_exact_async(&'_ mut self, sz: usize) -> GrantExactFuture<'a, '_, B, SUBS> {
        GrantExactFuture {
            publisher: self,
            sz,
        }
    }

    /// Async version of [Self::grant_max_remaining].
    /// Will wait for the buffer to at least 1 byte available, as soon as it does, return the grant.
    pub fn grant_max_remaining_async(
        &'_ mut self,
        sz: usize,
    ) -> GrantMaxRemainingFuture<'a, '_, B, SUBS> {
        GrantMaxRemainingFuture {
            publisher: self,
            sz,
        }
    }
}

impl<'a, B, const SUBS: usize> Drop for Publisher<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();
            inner.unregister_publisher();
        })
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
pub struct GrantW<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    pub(crate) buf: &'a mut [u8],
    channel: &'a Mutex<NoopRawMutex, RefCell<PubSubState<B, SUBS>>>,
    pub(crate) to_commit: usize,
}

unsafe impl<'a, B: BufferProvider, const SUBS: usize> Send for GrantW<'a, B, SUBS> {}

impl<'a, B, const SUBS: usize> GrantW<'a, B, SUBS>
where
    B: BufferProvider,
{
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
    /// # // PubSubState test shim!
    /// # fn channeltest() {
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let mut buffer: PubSubState<StaticBufferProvider<6>> = PubSubState::new_static();
    /// let (mut prod, mut cons) = buffer.try_split().unwrap();
    ///
    /// // Successfully obtain and commit a grant of four bytes
    /// let mut grant = prod.grant_max_remaining(4).unwrap();
    /// grant.buf().copy_from_slice(&[1, 2, 3, 4]);
    /// grant.commit(4);
    /// # // PubSubState test shim!
    /// # }
    /// #
    /// # fn main() {
    /// # #[cfg(not(feature = "thumbv6"))]
    /// # channeltest();
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
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();
            // If there is no grant in progress, return early. This
            // generally means we are dropping the grant within a
            // wrapper structure
            if !inner.write_in_progress {
                return;
            }

            // Writer component. Must never write to READ,
            // be careful writing to LAST

            // Saturate the grant commit
            let len = self.buf.len();
            let used = min(len, used);

            inner.reserve -= len - used;

            let max = len;
            let new_write = inner.reserve;

            if (new_write < inner.write) && (inner.write != max) {
                // We have already wrapped, but we are skipping some bytes at the end of the ring.
                // Mark `last` where the write pointer used to be to hold the line here
                inner.last = inner.write;
            } else if new_write > inner.last {
                // We're about to pass the last pointer, which was previously the artificial
                // end of the ring. Now that we've passed it, we can "unlock" the section
                // that was previously skipped.
                //
                // Since new_write is strictly larger than last, it is safe to move this as
                // the other thread will still be halted by the (about to be updated) write
                // value
                inner.last = max;
            }
            // else: If new_write == last, either:
            // * last == max, so no need to write, OR
            // * If we write in the end chunk again, we'll update last to max next time
            // * If we write to the start chunk in a wrap, we'll update last when we
            //     move write backwards

            // Write must be updated AFTER last, otherwise read could think it was
            // time to invert early!
            inner.write = new_write;

            // Allow subsequent grants
            inner.write_in_progress = false;

            inner.subscriber_wakers.wake();
        })
    }

    /// Configures the amount of bytes to be commited on drop.
    pub fn to_commit(&mut self, amt: usize) {
        self.to_commit = self.buf.len().min(amt);
    }
}

impl<'a, B, const SUBS: usize> Drop for GrantW<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.commit_inner(self.to_commit)
    }
}

impl<'a, B, const SUBS: usize> Deref for GrantW<'a, B, SUBS>
where
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buf
    }
}

impl<'a, B, const SUBS: usize> DerefMut for GrantW<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        self.buf
    }
}

/// Future returned [Publisher::grant_exact_async]
pub struct GrantExactFuture<'a, 'b, B, const SUBS: usize>
where
    B: BufferProvider,
{
    publisher: &'b mut Publisher<'a, B, SUBS>,
    sz: usize,
}

impl<'a, 'b, B, const SUBS: usize> Future for GrantExactFuture<'a, 'b, B, SUBS>
where
    B: BufferProvider,
{
    type Output = Result<GrantW<'a, B, SUBS>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Check if it's event  possible to get the requested size
        // Ex:
        // [0|1|2|3|4|5|6|7|8]
        //              ^
        //              Write pointer
        // Check if the buffer from 6 to 8 satisfies or if the buffer from 0 to 5 does.
        // If so, create the future, if not, we need the return since the future will never resolve.
        // Ideally, we could just wait for all the read to complete and reset the read and write to 0, but that is currently not supported

        if self.publisher.channel.lock(|channel| {
            let inner = channel.borrow();
            let max = inner.capacity();
            self.sz > max || (self.sz > max - inner.write && self.sz >= inner.write)
        }) {
            return Poll::Ready(Err(Error::InsufficientSize));
        }

        let sz = self.sz;

        match self.publisher.grant_exact(sz) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(e) => match e {
                Error::GrantInProgress | Error::InsufficientSize => {
                    self.publisher.channel.lock(|channel| {
                        let mut inner = channel.borrow_mut();
                        inner.publisher_waker.register(cx.waker());
                    });
                    Poll::Pending
                }
                _ => Poll::Ready(Err(e)),
            },
        }
    }
}

impl<'a, 'b, B: BufferProvider, const SUBS: usize> Unpin for GrantExactFuture<'a, 'b, B, SUBS> {}

/// Future returned [Publisher::grant_max_remaining_async]
pub struct GrantMaxRemainingFuture<'a, 'b, B, const SUBS: usize>
where
    B: BufferProvider,
{
    publisher: &'b mut Publisher<'a, B, SUBS>,
    sz: usize,
}

impl<'a, 'b, B, const SUBS: usize> Future for GrantMaxRemainingFuture<'a, 'b, B, SUBS>
where
    B: BufferProvider,
{
    type Output = Result<GrantW<'a, B, SUBS>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let sz = self.sz;

        match self.publisher.grant_max_remaining(sz) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(e) => match e {
                Error::GrantInProgress | Error::InsufficientSize => {
                    self.publisher.channel.lock(|channel| {
                        let mut inner = channel.borrow_mut();
                        inner.publisher_waker.register(cx.waker());
                    });
                    Poll::Pending
                }
                _ => Poll::Ready(Err(e)),
            },
        }
    }
}

impl<'a, 'b, B: BufferProvider, const SUBS: usize> Unpin for GrantMaxRemainingFuture<'a, 'b, B, SUBS> {}
