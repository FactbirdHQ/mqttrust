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

/// A Publisher of Framed data
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

    /// Receive a grant for a frame with a maximum size of `max_sz` in bytes.
    ///
    /// This size does not include the size of the frame header. The exact size
    /// of the frame can be set on `commit`.
    pub fn grant(&mut self, max_sz: usize) -> Result<FrameGrantW<'a, B, SUBS>> {
        self.grant_with_context(max_sz, None)
    }

    pub fn grant_with_context(
        &mut self,
        mut max_sz: usize,
        cx: Option<&mut Context<'_>>,
    ) -> Result<FrameGrantW<'a, B, SUBS>> {
        max_sz += Header::subs_header_len::<SUBS>();

        let frame_hdr_len = Header::encoded_len(max_sz);
        let sz = max_sz + frame_hdr_len;

        let inner = self.channel;

        if inner.write_in_progress.swap(true, AcqRel) {
            if let Some(cx) = cx {
                inner.publisher_waker.register(cx.waker());
            }
            return Err(Error::GrantInProgress);
        }

        // Writer component. Must never write to `read`,
        // be careful writing to `load`
        let write = inner.write.load(Acquire);
        let read = inner.read.load(Acquire);
        let max = inner.capacity;
        let already_inverted = write < read;

        let start = if already_inverted {
            if (write + sz) < read {
                // Inverted, room is still available
                write
            } else {
                // Inverted, no room is available
                inner.write_in_progress.store(false, Release);
                if let Some(cx) = cx {
                    inner.publisher_waker.register(cx.waker());
                }
                return Err(Error::InsufficientSize);
            }
        } else if write + sz <= max {
            // Non inverted condition
            write
        } else {
            // Not inverted, but need to go inverted

            // NOTE: We check sz < read, NOT <=, because
            // write must never == read in an inverted condition, since
            // we will then not be able to tell if we are inverted or not
            if sz < read {
                // Invertible situation
                0
            } else {
                // Not invertible, no space
                inner.write_in_progress.store(false, Release);
                if let Some(cx) = cx {
                    inner.publisher_waker.register(cx.waker());
                }
                return Err(Error::InsufficientSize);
            }
        };

        // Safe write, only viewed by this task
        inner.reserve.store(start + sz, Release);

        // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
        // are all `#[repr(Transparent)]
        let start_of_buf_ptr = unsafe { (*inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
        let grant_slice = unsafe { from_raw_parts_mut(start_of_buf_ptr.add(start), sz) };

        Ok(FrameGrantW {
            buf: grant_slice,
            channel: self.channel,
            to_commit: 0,
            frame_hdr_len: frame_hdr_len as u8,
        })
    }

    /// Async version of [Self::grant]
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
        &self.buf[self.frame_hdr_len as usize + Header::subs_header_len::<SUBS>()..]
    }
}

impl<'a, B, const SUBS: usize> DerefMut for FrameGrantW<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.frame_hdr_len as usize + Header::subs_header_len::<SUBS>()..]
    }
}

impl<'a, B, const SUBS: usize> FrameGrantW<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    /// Finalizes a writable grant given by `grant()` or `grant_max()`.
    /// This makes the data available to be read via `read()`. This consumes
    /// the grant.
    ///
    /// If `used` is larger than the given grant, the maximum amount will
    /// be committed
    ///
    /// NOTE:  If the `thumbv6` feature is selected, this function takes a short critical
    /// section while committing.
    pub fn commit(mut self, used: usize) {
        let total_len = self.set_header(used);
        self.commit_inner(total_len);
        core::mem::forget(self);
    }

    /// Configures the amount of bytes to be committed on drop.
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
        &mut self.buf[self.frame_hdr_len as usize + Header::subs_header_len::<SUBS>()..]
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
            &mut self.buf[self.frame_hdr_len as usize + Header::subs_header_len::<SUBS>()..],
        )
    }

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

        let write = inner.write.load(Acquire);
        inner.reserve.fetch_sub(len - used, AcqRel);

        let max = inner.capacity;
        let last = inner.last.load(Acquire);
        let new_write = inner.reserve.load(Acquire);

        if (new_write < write) && (write != max) {
            // We have already wrapped, but we are skipping some bytes at the end of the ring.
            // Mark `last` where the write pointer used to be to hold the line here
            inner.last.store(write, Release);
        } else if new_write > last {
            // We're about to pass the last pointer, which was previously the artificial
            // end of the ring. Now that we've passed it, we can "unlock" the section
            // that was previously skipped.
            //
            // Since new_write is strictly larger than last, it is safe to move this as
            // the other thread will still be halted by the (about to be updated) write
            // value
            inner.last.store(max, Release);
        }
        // else: If new_write == last, either:
        // * last == max, so no need to write, OR
        // * If we write in the end chunk again, we'll update last to max next time
        // * If we write to the start chunk in a wrap, we'll update last when we
        //     move write backwards

        // Write must be updated AFTER last, otherwise read could think it was
        // time to invert early!
        inner.write.store(new_write, Release);

        // Allow subsequent grants
        inner.write_in_progress.store(false, Release);

        inner.subscriber_wakers.wake();
    }

    /// Set the header and return the total size
    fn set_header(&mut self, mut used: usize) -> usize {
        used += Header::subs_header_len::<SUBS>();

        // Saturate the commit size to the available frame size
        let grant_len = self.buf.len();
        let frame_hdr_len: usize = self.frame_hdr_len.into();
        let frame_len = min(used, grant_len - frame_hdr_len);
        let total_len = frame_len + frame_hdr_len;

        // Write the actual frame length to the header
        Header::encode_usize_to_slice(frame_len, frame_hdr_len, &mut self.buf[..frame_hdr_len]);
        // self.buf[frame_hdr_len..][..Header::subs_header_len::<SUBS>()].fill(0);

        total_len
    }
}
