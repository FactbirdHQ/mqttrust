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

use super::header::Header;
use super::{BufferProvider, Error, PubSubChannel};

use super::Result;

use bitmaps::{Bits, BitsImpl};
use portable_atomic::Ordering::{AcqRel, Acquire, Release};

use core::future::Future;
use core::pin::Pin;
use core::slice::from_raw_parts_mut;
use core::task::{Context, Poll};
use core::{
    cmp::min,
    ops::{Deref, DerefMut},
};

/// Provides a publishing interface for writing data frames into a shared
/// `PubSubChannel`.
///
/// This struct allows for reserving contigious regions of memory within the channel's
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
pub struct FramePublisher<'a, B, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    channel: &'a PubSubChannel<B, SUBS>,
}

impl<'a, B, const SUBS: usize> FramePublisher<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a PubSubChannel<B, SUBS>) -> Self {
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
    pub fn grant(&mut self, max_sz: usize) -> Result<FrameGrantW<'a, B, SUBS>> {
        self.grant_with_context(max_sz, None)
    }

    /// Returns the current amount of free contiguous space available in the channel.
    ///
    /// This represents the largest contiguous block of memory that can be
    /// immediately granted for writing. It does not include space that
    /// could become available after the next read operation.
    pub fn free_capacity(&self) -> usize {
        let inner = self.channel;
        let write = inner.write.load(Acquire);
        let read = inner.read.load(Acquire);
        let max = inner.capacity;

        if write < read {
            // Inverted case
            read - write
        } else if write == read {
            // Empty or full, check reservation
            if inner.reserve.load(Acquire) == read {
                // Empty
                max
            } else {
                // Full
                0
            }
        } else {
            // Non-inverted case
            max - write + read
        }
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
    ) -> Result<FrameGrantW<'a, B, SUBS>> {
        // Calculate the total frame header length.
        let frame_hdr_len = Header::encoded_len(max_sz);

        // Calculate the total size required for the frame, including header.
        let sz = max_sz + frame_hdr_len;

        // Get a reference to the inner channel.
        let inner = self.channel;

        // Check if a write operation is already in progress.
        // If so, register the waker (if provided) and return an error.
        if inner.write_in_progress.swap(true, AcqRel) {
            if let Some(cx) = cx {
                inner.publisher_waker.register(cx.waker());
            }
            return Err(Error::GrantInProgress);
        }

        // Acquire the write, read pointers and the buffer capacity.
        // Writer component. Must never write to `read`,
        // be careful writing to `load`
        let write = inner.write.load(Acquire);
        let read = inner.read.load(Acquire);
        let max = inner.capacity;

        // Check if the buffer is already in an inverted state (write < read).
        let already_inverted = write < read;

        // Determine the starting point for writing the new data.
        let start = if already_inverted {
            // Inverted case.
            if (write + sz) < read {
                // Enough space available in the inverted buffer.
                write
            } else {
                // Not enough space, even in inverted mode.
                inner.write_in_progress.store(false, Release);
                if let Some(cx) = cx {
                    inner.publisher_waker.register(cx.waker());
                }
                return Err(Error::InsufficientSize);
            }
        } else if write + sz <= max {
            // Non-inverted case with enough space.
            write
        } else {
            // Non-inverted, but needs to become inverted.

            // NOTE: We check sz < read, NOT <=, because
            // write must never == read in an inverted condition, since
            // we will then not be able to tell if we are inverted or not
            if sz <= read {
                // Enough space to invert.
                inner.last.store(write, Release); // Set 'last' to the current write position
                0 // Start writing from the beginning
            } else {
                // Not enough space to invert.
                inner.write_in_progress.store(false, Release);
                if let Some(cx) = cx {
                    inner.publisher_waker.register(cx.waker());
                }
                return Err(Error::InsufficientSize);
            }
        };

        // Calculate the new reservation point and store it atomically.
        // Safe write, only viewed by this task
        inner.reserve.store(start + sz, Release);

        // Get a mutable slice of the buffer for the granted memory.
        // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
        // are all `#[repr(Transparent)]
        let start_of_buf_ptr = unsafe { (*inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
        let grant_slice = unsafe { from_raw_parts_mut(start_of_buf_ptr.add(start), sz) };

        // Create and return a FrameGrantW representing the granted memory.
        Ok(FrameGrantW {
            buf: grant_slice,
            channel: self.channel,
            to_commit: 0,
            frame_hdr_len: frame_hdr_len as u8,
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
    ///
    ///     // Write data to the granted slice...
    ///
    ///     // Commit the written data to the buffer.
    ///     grant.commit(written_size);
    /// };
    /// ```
    pub fn grant_async(&'_ mut self, max_sz: usize) -> GrantFuture<'a, '_, B, SUBS> {
        GrantFuture {
            publisher: self,
            sz: max_sz,
        }
    }
}

impl<'a, B, const SUBS: usize> Drop for FramePublisher<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.channel.publisher_taken.store(false, Release);
    }
}

/// Future returned [FramePublisher::grant_async]
#[must_use]
pub struct GrantFuture<'a, 'b, B, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    publisher: &'b mut FramePublisher<'a, B, SUBS>,
    sz: usize,
}

impl<'a, 'b, B, const SUBS: usize> Future for GrantFuture<'a, 'b, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    type Output = Result<FrameGrantW<'a, B, SUBS>>;

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
        let max = self.publisher.channel.capacity;
        let write = self.publisher.channel.write.load(Acquire);
        if self.sz > max || (self.sz > max - write && self.sz >= write) {
            return Poll::Ready(Err(Error::InsufficientSize));
        }

        let sz = self.sz;

        match self.publisher.grant_with_context(sz, Some(cx)) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(e) => match e {
                Error::GrantInProgress | Error::InsufficientSize => Poll::Pending,
                _ => Poll::Ready(Err(e)),
            },
        }
    }
}

impl<'a, 'b, B: BufferProvider, const SUBS: usize> Unpin for GrantFuture<'a, 'b, B, SUBS> where
    BitsImpl<{ SUBS }>: Bits
{
}

/// A write grant for a single frame
///
/// NOTE: If the grant is dropped without explicitly committing
/// the contents without first calling `to_commit()`, then no
/// frame will be committed for writing.
pub struct FrameGrantW<'a, B, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    pub(crate) buf: &'a mut [u8],
    channel: &'a PubSubChannel<B, SUBS>,
    pub(crate) to_commit: usize,
    frame_hdr_len: u8,
}

impl<'a, B, const SUBS: usize> Drop for FrameGrantW<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.commit_inner(self.to_commit)
    }
}

impl<'a, B, const SUBS: usize> Deref for FrameGrantW<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.buf[self.frame_hdr_len as usize..]
    }
}

impl<'a, B, const SUBS: usize> DerefMut for FrameGrantW<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.frame_hdr_len as usize..]
    }
}

impl<'a, B, const SUBS: usize> FrameGrantW<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
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
    /// # Important Note:
    ///
    /// - **Critical Section:** If you are compiling for targets without atomic
    /// CAS operations, this function will briefly enter a critical section
    /// while it updates the internal state of the ring buffer.
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
        let total_len = self.set_header(used);
        self.commit_inner(total_len);
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
        if amt == 0 {
            self.to_commit = 0;
        } else {
            let size = self.set_header(amt);
            self.to_commit = self.buf.len().min(size);
        }
    }

    /// Obtain access to the inner buffer for writing
    pub fn buf(&mut self) -> &mut [u8] {
        &mut self.buf[self.frame_hdr_len as usize..]
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
        core::mem::transmute::<&mut [u8], &'static mut [u8]>(
            &mut self.buf[self.frame_hdr_len as usize..],
        )
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
        let inner = self.channel;

        // If there is no grant in progress, return early. This
        // generally means we are dropping the grant within a
        // wrapper structure
        if !inner.write_in_progress.load(Acquire) {
            return;
        }

        // Writer component. Must never write to READ,
        // be careful writing to LAST

        // Saturate the grant commit
        let len = self.buf.len();
        let used = min(len, used);

        // Get current write pointer and update the reservation to reflect used bytes
        let write = inner.write.load(Acquire);
        inner.reserve.fetch_sub(len - used, AcqRel);

        // Load the ring buffer boundaries and the new write pointer
        let max = inner.capacity;
        let last = inner.last.load(Acquire);
        let new_write = inner.reserve.load(Acquire);

        if (new_write < write) && (write != max) {
            // We have already wrapped around the buffer, but we are skipping some bytes
            // at the end of the ring (due to a previous partial commit).
            // Update 'last' to the previous write pointer to mark the end of usable space
            // before the wrapped region.
            inner.last.store(write, Release);
        } else if new_write > last {
            // We're about to pass the last pointer, which was previously the artificial
            // end of the ring. This means we've filled the gap and can now use the
            // entire buffer capacity. So, update 'last' to the maximum capacity.
            inner.last.store(max, Release);
        }
        // else: Cases where we don't need to update 'last':
        //   * new_write == last && last == max:  We're at the very end, no need to update.
        //   * new_write == last && last != max:  We'll update 'last' in a future commit when needed.

        // Important: Update the write pointer *after* updating 'last'.
        // This ensures that the reader doesn't incorrectly assume it can
        // invert the buffer before the writer has finished updating 'last'.
        inner.write.store(new_write, Release);

        // Allow subsequent grants as the write operation is complete.
        inner.write_in_progress.store(false, Release);

        // Wake up any subscribers waiting for data.
        inner.subscriber_wakers.wake();
    }

    /// Set the header and return the total size
    fn set_header(&mut self, used: usize) -> usize {
        // Saturate the commit size to the available frame size
        let grant_len = self.buf.len();
        let frame_hdr_len: usize = self.frame_hdr_len.into();
        let frame_len = min(used, grant_len - frame_hdr_len);
        let total_len = frame_len + frame_hdr_len;

        // Write the actual frame length to the header
        Header::encode_usize_to_slice(frame_len, frame_hdr_len, &mut self.buf[..frame_hdr_len]);

        total_len
    }
}
