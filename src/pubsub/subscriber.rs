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

use crate::pubsub::header::Header;
use crate::pubsub::{Error, MessageInfo};
use crate::topic_filter::TopicFilter;

use super::{BufferProvider, Message, PubSubChannel};

use super::Result;

use bitmaps::{Bitmap, Bits, BitsImpl};
use portable_atomic::Ordering::{AcqRel, Acquire, Release};

use core::future::Future;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::slice::from_raw_parts_mut;
use core::task::{Context, Poll};

/// A subscriber of framed MQTT data
pub struct FrameSubscriber<'a, B, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    id: usize,
    channel: &'a PubSubChannel<B, SUBS>,
}

impl<'a, B, const SUBS: usize> FrameSubscriber<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a PubSubChannel<B, SUBS>, id: usize) -> Self {
        Self { channel, id }
    }

    /// Obtain the next available MQTT message matching topic_filters, if any
    pub fn read_message(&mut self, topic_filters: &[TopicFilter]) -> Result<Message<'a, B, SUBS>> {
        self.read_message_with_context(topic_filters, None)
    }

    pub fn read_message_with_context(
        &mut self,
        topic_filters: &[TopicFilter],
        cx: Option<&mut Context<'_>>,
    ) -> Result<Message<'a, B, SUBS>> {
        let mut grant = match self.read_inner() {
            Ok(grant) => grant,
            Err(e) => {
                if let Some(cx) = cx {
                    self.channel.subscriber_wakers.register(cx.waker());
                }
                return Err(e);
            }
        };

        if let Some(info) = MessageInfo::try_new(&grant) {
            if topic_filters
                .iter()
                .any(|f| f.is_match(info.topic_name(&grant)))
            {
                return Ok(info.to_message(grant));
            }

            let msg_bitmap = grant.mark_touched(self.id);

            // Check if message has been touched by all subscribers!
            if self
                .channel
                .subscribers_taken
                .lock(|subs_bitmap| (msg_bitmap & *subs_bitmap.borrow()) == *subs_bitmap.borrow())
            {
                grant.release();
            }
        }

        if let Some(cx) = cx {
            self.channel.subscriber_wakers.register(cx.waker());
        }

        Err(Error::MismatchedTopicFilter)
    }

    /// Async version of [Self::read_message].
    /// Will wait for the buffer to have data to read. When data is available, the grant is returned.
    pub fn read_message_async<'b>(
        &'b mut self,
        topic_filters: &'b [TopicFilter],
    ) -> MessageReadFuture<'a, 'b, B, SUBS> {
        MessageReadFuture {
            sub: self,
            topic_filters,
        }
    }

    /// Obtain the next available MQTT frame, if any
    pub fn read_any(&mut self) -> Result<FrameGrantR<'a, B, SUBS>> {
        self.read_any_with_context(None)
    }

    pub fn read_any_with_context(
        &mut self,
        cx: Option<&mut Context<'_>>,
    ) -> Result<FrameGrantR<'a, B, SUBS>> {
        match self.read_inner() {
            Ok(grant) => Ok(grant),
            Err(e) => {
                if let Some(cx) = cx {
                    self.channel.subscriber_wakers.register(cx.waker());
                }
                return Err(e);
            }
        }
    }

    /// Async version of [Self::read].
    /// Will wait for the buffer to have data to read. When data is available, the grant is returned.
    pub fn read_any_async<'b>(&'b mut self) -> GrantReadFuture<'a, 'b, B, SUBS> {
        GrantReadFuture { sub: self }
    }

    fn read_inner(&mut self) -> Result<FrameGrantR<'a, B, SUBS>> {
        let inner = self.channel;

        if inner.read_in_progress.swap(true, AcqRel) {
            return Err(Error::GrantInProgress);
        }

        let write = inner.write.load(Acquire);
        let last = inner.last.load(Acquire);
        let mut read = inner.read.load(Acquire);

        // Resolve the inverted case or end of read
        if (read == last) && (write < read) {
            read = 0;
            // This has some room for error, the other thread reads this
            // Impact to Grant:
            //   Grant checks if read < write to see if inverted. If not inverted, but
            //     no space left, Grant will initiate an inversion, but will not trigger it
            // Impact to Commit:
            //   Commit does not check read, but if Grant has started an inversion,
            //   grant could move Last to the prior write position
            // MOVING READ BACKWARDS!
            inner.read.store(0, Release);
        }

        let sz = if write < read {
            // Inverted, only believe last
            last
        } else {
            // Not inverted, only believe write
            write
        } - read;

        if sz == 0 {
            inner.read_in_progress.store(false, Release);
            return Err(Error::InsufficientSize);
        }

        // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
        // are all `#[repr(Transparent)]
        let start_of_buf_ptr = unsafe { (*inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
        let mut grant_slice = unsafe { from_raw_parts_mut(start_of_buf_ptr.add(read), sz) };

        // Additionally, we never commit less than a full frame with
        // a header, so if we have ANY data, we'll have a full header
        // and frame. [`FrameSubscriber::read`] will return an Error when
        // there are 0 bytes available.

        // The header consists of a single usize, encoded in native
        // endianness order
        let frame_len = Header::decode_usize(&grant_slice);
        let hdr_len = Header::decoded_len(grant_slice[0]);
        let total_len = frame_len + hdr_len;
        let hdr_len = hdr_len as u8;

        debug_assert!(grant_slice.len() >= total_len);

        // Reduce the grant down to the size of the frame with a header
        let mut new_buf: &mut [u8] = &mut [];
        core::mem::swap(&mut grant_slice, &mut new_buf);
        let (new, _) = new_buf.split_at_mut(total_len);
        grant_slice = new;

        Ok(FrameGrantR {
            buf: grant_slice,
            channel: self.channel,
            to_release: 0,
            hdr_len,
        })
    }
}

impl<'a, B, const SUBS: usize> Drop for FrameSubscriber<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    fn drop(&mut self) {
        let inner = self.channel;
        inner
            .subscribers_taken
            .lock(|f| f.borrow_mut().set(self.id, false));
    }
}

/// Future returned [Subscriber::read_async]
pub struct GrantReadFuture<'a, 'b, B, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    pub(crate) sub: &'b mut FrameSubscriber<'a, B, SUBS>,
}

impl<'a, 'b, B, const SUBS: usize> Future for GrantReadFuture<'a, 'b, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    type Output = Result<FrameGrantR<'a, B, SUBS>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.sub.read_any_with_context(Some(cx)) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(_) => Poll::Pending,
        }
    }
}

impl<'a, 'b, B: BufferProvider, const SUBS: usize> Unpin for GrantReadFuture<'a, 'b, B, SUBS> where
    BitsImpl<{ SUBS }>: Bits
{
}

/// Future returned [Subscriber::read_async]
pub struct MessageReadFuture<'a, 'b, B, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    pub(crate) sub: &'b mut FrameSubscriber<'a, B, SUBS>,
    pub(crate) topic_filters: &'b [TopicFilter],
}

impl<'a, 'b, B, const SUBS: usize> Future for MessageReadFuture<'a, 'b, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    type Output = Result<Message<'a, B, SUBS>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { topic_filters, sub } = self.get_mut();

        match sub.read_message_with_context(topic_filters, Some(cx)) {
            Ok(message) => Poll::Ready(Ok(message)),
            Err(_) => Poll::Pending,
        }
    }
}

impl<'a, 'b, B: BufferProvider, const SUBS: usize> Unpin for MessageReadFuture<'a, 'b, B, SUBS> where
    BitsImpl<{ SUBS }>: Bits
{
}

/// A read grant for a single frame
///
/// NOTE: If the grant is dropped without explicitly releasing
/// the contents, then no frame will be released.
pub struct FrameGrantR<'a, B, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    pub(crate) buf: &'a mut [u8],
    pub(crate) channel: &'a PubSubChannel<B, SUBS>,
    pub(crate) to_release: usize,
    pub(crate) hdr_len: u8,
}

impl<'a, B, const SUBS: usize> Deref for FrameGrantR<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.buf[self.hdr_len as usize + Header::subs_header_len::<SUBS>()..]
    }
}

impl<'a, B, const SUBS: usize> DerefMut for FrameGrantR<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.hdr_len as usize + Header::subs_header_len::<SUBS>()..]
    }
}

impl<'a, B, const SUBS: usize> FrameGrantR<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    /// Obtain access to the inner buffer for reading
    pub fn buf(&self) -> &[u8] {
        &self.buf[self.hdr_len as usize + Header::subs_header_len::<SUBS>()..]
    }

    /// Obtain mutable access to the read grant
    ///
    /// This is useful if you are performing in-place operations
    /// on an incoming packet, such as decryption
    pub fn buf_mut(&mut self) -> &mut [u8] {
        &mut self.buf[self.hdr_len as usize + Header::subs_header_len::<SUBS>()..]
    }

    /// Sometimes, it's not possible for the lifetimes to check out. For example,
    /// if you need to hand this buffer to a function that expects to receive a
    /// `&'static [u8]`, it is not possible for the inner reference to outlive the
    /// grant itself.
    ///
    /// You MUST guarantee that in no cases, the reference that is returned here outlives
    /// the grant itself. Once the grant has been released, referencing the data contained
    /// WILL cause undefined behavior.
    ///
    /// Additionally, you must ensure that a separate reference to this data is not created
    /// to this data, e.g. using `Deref` or the `buf()` method of this grant.
    pub unsafe fn as_static_buf(&self) -> &'static [u8] {
        core::mem::transmute::<&[u8], &'static [u8]>(
            &self.buf[self.hdr_len as usize + Header::subs_header_len::<SUBS>()..],
        )
    }

    /// Release a frame to make the space available for future writing
    ///
    /// Note: The full frame is always released
    pub fn release(mut self) {
        // For a read grant, we have already shrunk the grant
        // size down to the correct size
        let len = self.buf.len();
        self.release_inner(len);
        core::mem::forget(self);
    }

    /// Set whether the read frame should be automatically released
    pub fn auto_release(&mut self, is_auto: bool) {
        self.to_release = self.buf.len().min(if is_auto { self.buf.len() } else { 0 });
    }

    fn release_inner(&mut self, used: usize) {
        let inner = self.channel;

        // If there is no grant in progress, return early. This
        // generally means we are dropping the grant within a
        // wrapper structure
        if !inner.read_in_progress.load(Acquire) {
            return;
        }

        // This should always be checked by the public interfaces
        debug_assert!(used <= self.buf.len());

        // This should be fine, purely incrementing
        let _ = inner.read.fetch_add(used, Release);

        inner.read_in_progress.store(false, Release);

        inner.publisher_waker.wake();
    }

    fn mark_touched(&mut self, id: usize) -> Bitmap<SUBS> {
        let mut msg_bitmap = bitmaps::Bitmap::try_from(
            &self.buf[self.hdr_len as usize..][..Header::subs_header_len::<SUBS>()],
        )
        .unwrap();
        msg_bitmap.set(id, true);

        self.buf[self.hdr_len as usize..][..Header::subs_header_len::<SUBS>()]
            .copy_from_slice(msg_bitmap.as_bytes());

        msg_bitmap
    }
}

impl<'a, B, const SUBS: usize> Drop for FrameGrantR<'a, B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.release_inner(self.to_release)
    }
}

#[cfg(test)]
mod tests {
    use crate::pubsub::StaticBufferProvider;

    use super::*;

    #[test_log::test]
    fn update_subscriber_map() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 10> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();
        let mut subscriber2 = pubsub.subscriber().unwrap();

        let data = [
            48, 10, // Header (publish)
            0, 4, // Topic length
            116, 101, 115, 116, // Topic (test)
            0,   // Properties
            116, 101, 115, 116, // Payload (test)
        ];

        let mut wgr = publisher.grant(data.len()).unwrap();
        wgr[..data.len()].copy_from_slice(&data);

        wgr.commit(data.len());

        assert!(subscriber.read_message(&[]).is_err());

        let rgr = subscriber.read_any().unwrap();
        drop(rgr);

        assert!(subscriber2.read_message(&[]).is_err());

        assert!(subscriber.read_any().is_err())
    }
}
