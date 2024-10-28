//! A Framed flavor of [`PubSubChannel`], useful for variable length packets
//!
//! This module allows for a `Framed` mode of operation,
//! where a size header is included in each grant, allowing for
//! "chunks" of data to be passed through a [`PubSubChannel`], rather than
//! just a stream of bytes. This is convenient when receiving
//! packets of variable sizes.
//!
//! ## Frame header
//!
//! An internal header is required for each frame stored
//! inside of the [`PubSubChannel`]. This header is never exposed to end
//! users of the pubsub library.
//!
//! A variable sized integer is used for the header size, and the
//! size of this header is based on the max size requested for the grant.
//! This header size must be factored in when calculating an appropriate
//! total size of your buffer.
//!
//! Even if a smaller portion of the grant is committed, the original
//! requested grant size will be used for the header size calculation.
//!
//! For example, if you request a 128 byte grant, the header size will
//! be two bytes. If you then only commit 32 bytes, two bytes will still
//! be used for the header of that grant.
//!
//! | Grant Size (bytes)    | Header size (bytes)  |
//! | :---                  | :---                 |
//! | 1..(2^7)              | 1                    |
//! | (2^7)..(2^14)         | 2                    |
//! | (2^14)..(2^21)        | 3                    |
//! | (2^21)..(2^28)        | 4                    |
//! | (2^28)..(2^35)        | 5                    |
//! | (2^35)..(2^42)        | 6                    |
//! | (2^42)..(2^49)        | 7                    |
//! | (2^49)..(2^56)        | 8                    |
//! | (2^56)..(2^64)        | 9                    |
//!

use super::{BufferProvider, Error, Header, PubSubChannel};

use super::Result;

use embassy_sync::blocking_mutex::raw::RawMutex;

use core::future::Future;
use core::pin::Pin;
use core::ptr::NonNull;
use core::slice::from_raw_parts_mut;
use core::task::{Context, Poll};
use core::{
    cmp::min,
    ops::{Deref, DerefMut},
};

/// Provides a publishing interface for writing data frames into a shared
/// `PubSubChannel`.
///
/// This struct allows for reserving contiguous regions of memory within the channel's
/// buffer, writing data into them, and then committing the data to be read
/// by subscribers.
///
/// # Type Parameters
///
/// * `'a`:  The lifetime of the reference to the `PubSubChannel`. The
///   `FramePublisher` cannot outlive the channel it references.
/// * `B`: The type of buffer used by the `PubSubChannel`.
/// * `SUBS`: The maximum number of subscribers that can be attached
///   to the `PubSubChannel`.
///
/// # Example
///
/// ```ignore
/// // Assuming 'channel' is a PubSubChannel<MyBuffer, 16>
/// let publisher: FramePublisher<_, 16> = channel.publisher().unwrap();
///
/// // Now use 'publisher' to grant memory and write frames...
/// ```
pub struct FramePublisher<'a, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    channel: &'a PubSubChannel<M, B, SUBS>,
}

impl<'a, M, B, const SUBS: usize> FramePublisher<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a PubSubChannel<M, B, SUBS>) -> Self {
        Self { channel }
    }

    /// Attempts to immediately grant a region of memory for writing a frame.
    ///
    /// This function tries to reserve a contiguous region of memory within the
    /// internal buffer, large enough to hold a frame of at most `max_sz` bytes
    /// (not including the frame header).
    ///
    /// Unlike `grant_with_context`, this function does not take a `Context`
    /// argument and will not wait if the grant cannot be immediately fulfilled.
    /// Instead, it will return an `Err(Error::GrantInProgress)` if another
    /// write operation is in progress or `Err(Error::InsufficientSize)` if
    /// there's not enough space.
    ///
    /// # Returns
    ///
    /// * `Ok(FrameGrantW)` if a grant is successfully obtained.
    /// * `Err(Error::GrantInProgress)` if another write operation is currently
    ///   in progress.
    /// * `Err(Error::InsufficientSize)` if there is not enough contiguous
    ///   space available in the buffer to fulfill the request.
    pub fn grant(&mut self, max_sz: usize) -> Result<FrameGrantW<'a, M, B, SUBS>> {
        self.grant_with_context(max_sz, None)
    }

    pub fn grant_immediate(&mut self, max_sz: usize) -> Result<FrameGrantW<'a, M, B, SUBS>> {
        self.channel.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();

            while inner.free_space() < max_sz || inner.messages.is_full() {
                let message = inner.messages.pop_front();
                warn!("Force removed message! {:?}", message);
            }
        });

        self.grant(max_sz)
    }

    /// Attempts to grant a region of memory for writing a frame to.
    ///
    /// This function will attempt to reserve a contiguous region of memory
    /// within the internal buffer large enough to hold a frame of at most
    /// `max_sz` bytes (not including the frame header).
    ///
    /// The function takes an optional `Context` argument, which, if provided,
    /// will be used to register the publisher's waker in case the grant
    /// cannot be immediately fulfilled (e.g., due to insufficient space
    /// or an ongoing write operation).
    ///
    /// # Returns
    ///
    /// * `Ok(FrameGrantW)` if a grant is successfully obtained. The
    ///   `FrameGrantW` struct provides access to the granted memory slice
    ///   and other relevant information.
    ///
    /// * `Err(Error::GrantInProgress)` if another write operation is
    ///   currently in progress.
    ///
    /// * `Err(Error::InsufficientSize)` if there is not enough contiguous
    ///   space available in the buffer to fulfill the request, even after
    ///   considering potential buffer wrapping.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Attempt to obtain a grant for a frame of up to 1024 bytes.
    /// let mut grant = publisher.grant_with_context(1024, cx).unwrap();
    ///
    /// // Access the granted memory slice.
    /// let grant_slice = grant.buf;
    ///
    /// // Write data to the granted slice...
    ///
    /// // Commit the written data to the buffer.
    /// grant.commit(written_size);
    /// ```
    pub fn grant_with_context(
        &mut self,
        max_sz: usize,
        cx: Option<&mut Context<'_>>,
    ) -> Result<FrameGrantW<'a, M, B, SUBS>> {
        let start = self.channel.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();

            // Check if a write operation is already in progress.
            // If so, register the waker (if provided) and return an error.
            if inner.write_in_progress {
                if let Some(cx) = cx {
                    inner.publisher_waker.register(cx.waker());
                }
                return Err(Error::GrantInProgress);
            }

            if max_sz > inner.free_space() || inner.messages.is_full() {
                if let Some(cx) = cx {
                    inner.publisher_waker.register(cx.waker());
                }

                return Err(Error::InsufficientSize);
            }

            let new_start = match inner.messages.back() {
                Some(back) => {
                    // We have a back, so we know we also have a front, though they may be the same item
                    let front = inner.messages.front().unwrap();

                    let back_end = back.start + back.len as usize;

                    let inverted = back_end < front.start;

                    // We already checked that there is enough capacity in the
                    // channel, so we just need to determine if we should wrap
                    if inverted || max_sz <= inner.capacity - back_end {
                        back_end
                    } else {
                        0
                    }
                }
                None => 0,
            };

            inner.write_in_progress = true;

            Ok(new_start)
        })?;

        // Get a mutable slice of the buffer for the granted memory. This is
        // sound, as UnsafeCell, MaybeUninit, and GenericArray are all
        // `#[repr(Transparent)]
        let start_of_buf_ptr = unsafe { (*self.channel.buf.get()).buf().as_mut_ptr().cast::<u8>() };
        let grant_slice = unsafe { from_raw_parts_mut(start_of_buf_ptr.add(start), max_sz) };

        // Create and return a FrameGrantW representing the granted memory.
        Ok(FrameGrantW {
            buf: grant_slice.into(),
            channel: self.channel,
            start,
            to_commit: 0,
        })
    }

    /// Asynchronously attempts to grant a region of memory for writing a frame.
    ///
    /// This function provides a non-blocking way to obtain a memory grant
    /// for publishing a frame. It returns a `GrantFuture` that can be awaited
    /// to obtain the `FrameGrantW`.
    ///
    /// If a grant cannot be immediately fulfilled (e.g., due to insufficient
    /// space), the future will pend until space becomes available or the
    /// publisher is woken up by another task.
    ///
    /// # Returns
    ///
    /// A `GrantFuture` that resolves to a `Result<FrameGrantW<'a, B, SUBS>, Error>`:
    ///
    /// * `Ok(FrameGrantW)` if a grant is successfully obtained.
    /// * `Err(Error)` if an error occurs, such as insufficient buffer space.
    ///
    /// # Example
    ///
    /// ```ignore
    /// async {
    ///     // Asynchronously attempt to obtain a grant for a frame.
    ///     let mut grant = publisher.grant_async(1024).await.unwrap();
    ///
    ///     // Access the granted memory slice.
    ///     let grant_slice = grant.buf;
    ///     // Write data to the granted slice...
    ///
    ///     // Commit the written data to the buffer.
    ///     grant.commit(written_size);
    /// };
    /// ```
    pub fn grant_async(&mut self, max_sz: usize) -> GrantFuture<'a, '_, M, B, SUBS> {
        GrantFuture {
            publisher: self,
            sz: max_sz,
        }
    }

    /// Returns the number of elements currently in the channel.
    pub fn len(&self) -> usize {
        self.channel.len()
    }

    /// Returns whether the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.channel.is_empty()
    }

    /// Returns whether the channel is full.
    pub fn is_full(&self) -> bool {
        self.channel.is_full()
    }

    pub fn free_space(&self) -> usize {
        self.channel.free_space()
    }
}

impl<'a, M, B, const SUBS: usize> Drop for FramePublisher<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.channel
            .inner
            .lock(|inner| inner.borrow_mut().publisher_taken = false);
    }
}

/// Future returned [FramePublisher::grant_async]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct GrantFuture<'a, 'b, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    publisher: &'b mut FramePublisher<'a, M, B, SUBS>,
    sz: usize,
}

impl<'a, 'b, M, B, const SUBS: usize> Future for GrantFuture<'a, 'b, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    type Output = Result<FrameGrantW<'a, M, B, SUBS>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Check if it's even possible to get the requested size
        //
        // ```
        // Ex:
        // [0|1|2|3|4|5|6|7|8]
        //              ^
        //              Write pointer
        // ```
        //
        // Check if the buffer from 6 to 8 satisfies or if the buffer from 0 to
        // 5 does. If so, create the future, if not, we need the return since
        // the future will never resolve. Ideally, we could just wait for all
        // the read to complete and reset the read and write to 0, but that is
        // currently not supported
        // if let Err(e) = self.publisher.channel.inner.lock(|inner| {
        //     let state = inner.borrow();
        //     if self.sz > state.capacity
        //         || (self.sz > state.capacity - state.write && self.sz >= state.write)
        //     {
        //         return Err(Error::InsufficientSize);
        //     }
        //     Ok(())
        // }) {
        //     return Poll::Ready(Err(e));
        // }

        let sz = self.sz;

        match self.publisher.grant_with_context(sz, Some(cx)) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(Error::GrantInProgress | Error::InsufficientSize) => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl<'a, 'b, M: RawMutex, B: BufferProvider, const SUBS: usize> Unpin
    for GrantFuture<'a, 'b, M, B, SUBS>
{
}

/// A write grant for a single frame
///
/// NOTE: If the grant is dropped without explicitly committing
/// the contents without first calling `to_commit()`, then no
/// frame will be committed for writing.
pub struct FrameGrantW<'a, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    buf: NonNull<[u8]>,
    channel: &'a PubSubChannel<M, B, SUBS>,
    start: usize,
    to_commit: usize,
}

impl<'a, M, B, const SUBS: usize> Drop for FrameGrantW<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.commit_inner(self.to_commit)
    }
}

impl<'a, M, B, const SUBS: usize> Deref for FrameGrantW<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe { from_raw_parts_mut(self.buf.as_ptr() as *mut u8, self.buf.len()) }
    }
}

impl<'a, M, B, const SUBS: usize> DerefMut for FrameGrantW<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { from_raw_parts_mut(self.buf.as_ptr() as *mut u8, self.buf.len()) }
    }
}

impl<'a, M, B, const SUBS: usize> FrameGrantW<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    /// Finalizes a writable grant previously obtained with `grant()`, `grant_with_context()`,
    /// or `grant_async()`.
    ///
    /// This function is crucial for making the written data available to readers
    /// (`read()`).
    ///
    /// It writes the frame header (using the provided `used` length) and then commits
    /// the entire frame (header + data) to the underlying ring buffer.  After calling
    /// `commit()`, the grant is consumed and should no longer be used.
    ///
    /// # Arguments
    ///
    /// * `used` - The number of bytes actually used from the granted slice (excluding
    ///   the header). This value should be less than or equal to the length of the data
    ///   portion of the granted slice. If `used` is larger than the granted size, the
    ///   maximum granted size will be committed.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Assuming 'grant' is a FrameGrantW obtained from 'grant_with_context()'
    /// // or another grant function.
    ///
    /// // Write data into the 'grant.buf' slice. Let's say you used 'bytes_written' bytes.
    ///
    /// // Commit the written data:
    /// grant.commit(bytes_written);
    ///
    /// // The grant is now consumed.
    /// ```
    pub fn commit(mut self, used: usize) {
        self.commit_inner(used);
        core::mem::forget(self);
    }

    /// Configures the amount of data (including header) to be automatically committed when the `FrameGrantW` is dropped.
    ///
    /// This method allows you to specify a default commit size that will be applied if `commit()` is not
    /// explicitly called before the `FrameGrantW` goes out of scope. This can be useful for scenarios
    /// where you might partially fill a grant and want to ensure the written data is still published.
    ///
    /// # Arguments
    ///
    /// * `amt`:  The total number of bytes (including the header) to be committed when the `FrameGrantW`
    ///   is dropped.  This value should be less than or equal to the total size of the grant
    ///   (header size + data size).
    ///
    /// # Behavior
    ///
    /// * If `amt` is 0, the automatic commit on drop is disabled.
    /// * If `amt` is greater than 0, the header is written, and the amount to commit is set to the
    ///    minimum of `amt` and the total grant size.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut grant = publisher.grant(1024).unwrap();
    ///
    /// // Set a default commit size of 100 bytes:
    /// grant.to_commit(100);
    ///
    /// // Write data to grant.buf ...
    ///
    /// // ... no explicit call to `grant.commit()` is needed if the amount of data
    /// // written is less than or equal to 100 bytes.
    /// ```
    pub fn to_commit(&mut self, amt: usize) {
        self.to_commit = amt;
    }

    /// Obtain access to the inner buffer for writing
    pub fn buf(&mut self) -> &mut [u8] {
        self.deref_mut()
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
        core::mem::transmute::<&mut [u8], &'static mut [u8]>(self.deref_mut())
    }
    /// Commits a portion of the granted memory region.
    ///
    /// This function should be called after writing data to the granted memory
    /// slice to indicate that the written portion is ready to be published.
    ///
    /// # Arguments
    ///
    /// * `used` - The number of bytes actually used from the granted slice.
    ///   This value should be less than or equal to the length of the slice.
    fn commit_inner(&mut self, used: usize) {
        // Saturate the grant commit
        let used = min(self.buf.len(), used);

        self.channel.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();

            // If there is no grant in progress, return early. This
            // generally means we are dropping the grant within a
            // wrapper structure
            if !inner.write_in_progress {
                return;
            }

            if inner.subscriber_count == 0 {
                // We don't need to publish anything because there is no one to receive it
                return;
            }
            let header = Header {
                start: self.start,
                len: used as u16,
                message_id: inner.next_message_id,
                subscriber_count: inner.subscriber_count,
                read_in_progress: false,
            };

            if inner.messages.push_back(header).is_err() {
                // If we can't push the message, we can't commit the grant
                error!("Failed to commit grant! No room?!");
            };

            inner.next_message_id += 1;

            // Allow subsequent grants as the write operation is complete.
            inner.write_in_progress = false;

            // Wake up any subscribers waiting for data.
            inner.subscriber_wakers.wake();
        })
    }
}
