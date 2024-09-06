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

use super::{BufferProvider, GrantR, GrantW, PubSubChannel, Publisher, Subscriber};

use super::{
    vusize::{decode_usize, decoded_len, encode_usize_to_slice, encoded_len},
    Result,
};

use core::task::Context;
use core::{
    cmp::min,
    ops::{Deref, DerefMut},
};

/// A Publisher of Framed data
pub struct FramePublisher<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    pub(crate) publisher: Publisher<'a, B, SUBS>,
}

impl<'a, B, const SUBS: usize> FramePublisher<'a, B, SUBS>
where
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a PubSubChannel<B, SUBS>) -> Self {
        Self {
            publisher: Publisher::new(channel),
        }
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
        max_sz: usize,
        cx: Option<&mut Context<'_>>,
    ) -> Result<FrameGrantW<'a, B, SUBS>> {
        let hdr_len = encoded_len(max_sz);
        Ok(FrameGrantW {
            grant_w: self
                .publisher
                .grant_exact_with_context(max_sz + hdr_len, cx)?,
            hdr_len: hdr_len as u8,
        })
    }

    /// Async version of [Self::grant]
    pub async fn grant_async(&mut self, max_sz: usize) -> Result<FrameGrantW<'a, B, SUBS>> {
        let hdr_len = encoded_len(max_sz);
        Ok(FrameGrantW {
            grant_w: self.publisher.grant_exact_async(max_sz + hdr_len).await?,
            hdr_len: hdr_len as u8,
        })
    }
}

/// A Subscriber of Framed data
pub struct FrameSubscriber<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    pub(crate) subscriber: Subscriber<'a, B, SUBS>,
}

impl<'a, B, const SUBS: usize> FrameSubscriber<'a, B, SUBS>
where
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a PubSubChannel<B, SUBS>) -> Self {
        Self {
            subscriber: Subscriber::new(channel),
        }
    }

    /// Obtain the next available frame, if any
    pub fn read(&mut self) -> Option<FrameGrantR<'a, B, SUBS>> {
        self.read_with_context(None)
    }

    pub fn read_with_context(
        &mut self,
        cx: Option<&mut Context<'_>>,
    ) -> Option<FrameGrantR<'a, B, SUBS>> {
        // Get all available bytes. We never wrap a frame around,
        // so if a header is available, the whole frame will be.
        let mut grant_r = self.subscriber.read_with_context(cx).ok()?;

        // Additionally, we never commit less than a full frame with
        // a header, so if we have ANY data, we'll have a full header
        // and frame. `Subscriber::read` will return an Error when
        // there are 0 bytes available.

        // The header consists of a single usize, encoded in native
        // endianness order
        let frame_len = decode_usize(&grant_r);
        let hdr_len = decoded_len(grant_r[0]);
        let total_len = frame_len + hdr_len;
        let hdr_len = hdr_len as u8;

        debug_assert!(grant_r.len() >= total_len);

        // Reduce the grant down to the size of the frame with a header
        grant_r.shrink(total_len);

        Some(FrameGrantR { grant_r, hdr_len })
    }

    /// Async version of [Self::read]
    pub async fn read_async(&mut self) -> Result<FrameGrantR<'a, B, SUBS>> {
        // Get all available bytes. We never wrap a frame around,
        // so if a header is available, the whole frame will be.
        let mut grant_r = self.subscriber.read_async().await?;

        // Additionally, we never commit less than a full frame with
        // a header, so if we have ANY data, we'll have a full header
        // and frame. `Subscriber::read` will return an Error when
        // there are 0 bytes available.

        // The header consists of a single usize, encoded in native
        // endianness order
        let frame_len = decode_usize(&grant_r);
        let hdr_len = decoded_len(grant_r[0]);
        let total_len = frame_len + hdr_len;
        let hdr_len = hdr_len as u8;

        debug_assert!(grant_r.len() >= total_len);

        // Reduce the grant down to the size of the frame with a header
        grant_r.shrink(total_len);

        Ok(FrameGrantR { grant_r, hdr_len })
    }
}

/// A write grant for a single frame
///
/// NOTE: If the grant is dropped without explicitly committing
/// the contents without first calling `to_commit()`, then no
/// frame will be committed for writing.
pub struct FrameGrantW<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    grant_w: GrantW<'a, B, SUBS>,
    hdr_len: u8,
}

/// A read grant for a single frame
///
/// NOTE: If the grant is dropped without explicitly releasing
/// the contents, then no frame will be released.
pub struct FrameGrantR<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    pub(crate) grant_r: GrantR<'a, B, SUBS>,
    pub(crate) hdr_len: u8,
}

impl<'a, B, const SUBS: usize> Deref for FrameGrantW<'a, B, SUBS>
where
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.grant_w.buf[self.hdr_len.into()..]
    }
}

impl<'a, B, const SUBS: usize> DerefMut for FrameGrantW<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.grant_w.buf[self.hdr_len.into()..]
    }
}

impl<'a, B, const SUBS: usize> Deref for FrameGrantR<'a, B, SUBS>
where
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.grant_r.buf[self.hdr_len.into()..]
    }
}

impl<'a, B, const SUBS: usize> DerefMut for FrameGrantR<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.grant_r.buf[self.hdr_len.into()..]
    }
}

impl<'a, B, const SUBS: usize> FrameGrantW<'a, B, SUBS>
where
    B: BufferProvider,
{
    /// Commit a frame to make it available to the Subscriber half.
    ///
    /// `used` is the size of the payload, in bytes, not
    /// including the frame header
    pub fn commit(mut self, used: usize) {
        let total_len = self.set_header(used);

        // Commit the header + frame
        self.grant_w.commit(total_len);
    }

    /// Set the header and return the total size
    fn set_header(&mut self, used: usize) -> usize {
        // Saturate the commit size to the available frame size
        let grant_len = self.grant_w.len();
        let hdr_len: usize = self.hdr_len.into();
        let frame_len = min(used, grant_len - hdr_len);
        let total_len = frame_len + hdr_len;

        // Write the actual frame length to the header
        encode_usize_to_slice(frame_len, hdr_len, &mut self.grant_w[..hdr_len]);

        total_len
    }

    /// Configures the amount of bytes to be committed on drop.
    pub fn to_commit(&mut self, amt: usize) {
        if amt == 0 {
            self.grant_w.to_commit(0);
        } else {
            let size = self.set_header(amt);
            self.grant_w.to_commit(size);
        }
    }
}

impl<'a, B, const SUBS: usize> FrameGrantR<'a, B, SUBS>
where
    B: BufferProvider,
{
    /// Release a frame to make the space available for future writing
    ///
    /// Note: The full frame is always released
    pub fn release(mut self) {
        // For a read grant, we have already shrunk the grant
        // size down to the correct size
        let len = self.grant_r.len();
        self.grant_r.release_inner(len);
    }

    /// Set whether the read frame should be automatically released
    pub fn auto_release(&mut self, is_auto: bool) {
        self.grant_r
            .to_release(if is_auto { self.grant_r.len() } else { 0 });
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;

    use crate::pubsub::{PubSubChannel, StaticBufferProvider};

    #[test]
    fn frame_wrong_size() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> = PubSubChannel::new_static();
        let mut publisher = pubsub.framed_publisher().unwrap();
        let mut subscriber = pubsub.framed_subscriber().unwrap();

        // Create largeish grants
        let mut wgr = publisher.grant(127).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }
        // Note: In debug mode, this hits a debug_assert
        wgr.commit(256);

        let rgr = subscriber.read().unwrap();
        assert_eq!(rgr.len(), 127);
        for (i, by) in rgr.iter().enumerate() {
            assert_eq!((i as u8), *by);
        }
        rgr.release();
    }

    #[test]
    fn full_size() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> = PubSubChannel::new_static();
        let mut publisher = pubsub.framed_publisher().unwrap();
        let mut subscriber = pubsub.framed_subscriber().unwrap();
        let mut ctr = 0;

        for _ in 0..100 {
            // Create largeish grants
            if let Ok(mut wgr) = publisher.grant(127) {
                ctr += 1;
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = i as u8;
                }
                wgr.commit(127);

                let rgr = subscriber.read().unwrap();
                assert_eq!(rgr.len(), 127);
                for (i, by) in rgr.iter().enumerate() {
                    assert_eq!((i as u8), *by);
                }
                rgr.release();
            } else {
                // Create smallish grants
                let mut wgr = publisher.grant(1).unwrap();
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = i as u8;
                }
                wgr.commit(1);

                let rgr = subscriber.read().unwrap();
                assert_eq!(rgr.len(), 1);
                for (i, by) in rgr.iter().enumerate() {
                    assert_eq!((i as u8), *by);
                }
                rgr.release();
            };
        }

        assert!(ctr > 1);
    }

    #[test]
    fn frame_overcommit() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> = PubSubChannel::new_static();
        let mut publisher = pubsub.framed_publisher().unwrap();
        let mut subscriber = pubsub.framed_subscriber().unwrap();

        // Create largeish grants
        let mut wgr = publisher.grant(128).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }
        wgr.commit(255);

        let mut wgr = publisher.grant(64).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = (i as u8) + 128;
        }
        wgr.commit(127);

        let rgr = subscriber.read().unwrap();
        assert_eq!(rgr.len(), 128);
        rgr.release();

        let rgr = subscriber.read().unwrap();
        assert_eq!(rgr.len(), 64);
        rgr.release();
    }

    #[test]
    fn frame_undercommit() {
        let pubsub: PubSubChannel<StaticBufferProvider<512>, 1> = PubSubChannel::new_static();

        let mut publisher = pubsub.framed_publisher().unwrap();
        let mut subscriber = pubsub.framed_subscriber().unwrap();

        for _ in 0..100 {
            // Create largeish grants
            let mut wgr = publisher.grant(128).unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = i as u8;
            }
            wgr.commit(13);

            let mut wgr = publisher.grant(64).unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = (i as u8) + 128;
            }
            wgr.commit(7);

            let mut wgr = publisher.grant(32).unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = (i as u8) + 192;
            }
            wgr.commit(0);

            let rgr = subscriber.read().unwrap();
            assert_eq!(rgr.len(), 13);
            rgr.release();

            let rgr = subscriber.read().unwrap();
            assert_eq!(rgr.len(), 7);
            rgr.release();

            let rgr = subscriber.read().unwrap();
            assert_eq!(rgr.len(), 0);
            rgr.release();
        }
    }

    #[test]
    fn frame_auto_commit_release() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> = PubSubChannel::new_static();
        let mut publisher = pubsub.framed_publisher().unwrap();
        let mut subscriber = pubsub.framed_subscriber().unwrap();

        for _ in 0..100 {
            {
                let mut wgr = publisher.grant(64).unwrap();
                wgr.to_commit(64);
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = i as u8;
                }
                // drop
            }

            {
                let mut rgr = subscriber.read().unwrap();
                rgr.auto_release(true);
                let rgr = rgr;

                for (i, by) in rgr.iter().enumerate() {
                    assert_eq!(*by, i as u8);
                }
                assert_eq!(rgr.len(), 64);
                // drop
            }
        }

        assert!(subscriber.read().is_none());
    }

    #[test]
    fn async_frame_wrong_size() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> = PubSubChannel::new_static();
            let mut publisher = pubsub.framed_publisher().unwrap();
            let mut subscriber = pubsub.framed_subscriber().unwrap();

            // Create largeish grants
            let mut wgr = publisher.grant_async(127).await.unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = i as u8;
            }
            // Note: In debug mode, this hits a debug_assert
            wgr.commit(256);

            let rgr = subscriber.read_async().await.unwrap();
            assert_eq!(rgr.len(), 127);
            for (i, by) in rgr.iter().enumerate() {
                assert_eq!((i as u8), *by);
            }
            rgr.release();
        });
    }

    #[test]
    fn async_full_size() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> = PubSubChannel::new_static();
            let mut publisher = pubsub.framed_publisher().unwrap();
            let mut subscriber = pubsub.framed_subscriber().unwrap();

            let mut ctr = 0;

            for _ in 0..100 {
                // Create largeish grants
                if let Ok(mut wgr) = publisher.grant_async(127).await {
                    ctr += 1;
                    for (i, by) in wgr.iter_mut().enumerate() {
                        *by = i as u8;
                    }
                    wgr.commit(127);

                    let rgr = subscriber.read_async().await.unwrap();
                    assert_eq!(rgr.len(), 127);
                    for (i, by) in rgr.iter().enumerate() {
                        assert_eq!((i as u8), *by);
                    }
                    rgr.release();
                } else {
                    // Create smallish grants
                    let mut wgr = publisher.grant_async(1).await.unwrap();
                    for (i, by) in wgr.iter_mut().enumerate() {
                        *by = i as u8;
                    }
                    wgr.commit(1);

                    let rgr = subscriber.read_async().await.unwrap();
                    assert_eq!(rgr.len(), 1);
                    for (i, by) in rgr.iter().enumerate() {
                        assert_eq!((i as u8), *by);
                    }
                    rgr.release();
                };
            }

            assert!(ctr > 1);
        });
    }

    #[test]
    fn async_frame_overcommit() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> = PubSubChannel::new_static();
            let mut publisher = pubsub.framed_publisher().unwrap();
            let mut subscriber = pubsub.framed_subscriber().unwrap();

            // Create largeish grants
            let mut wgr = publisher.grant_async(128).await.unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = i as u8;
            }
            wgr.commit(255);

            let mut wgr = publisher.grant_async(64).await.unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = (i as u8) + 128;
            }
            wgr.commit(127);

            let rgr = subscriber.read_async().await.unwrap();
            assert_eq!(rgr.len(), 128);
            rgr.release();

            let rgr = subscriber.read_async().await.unwrap();
            assert_eq!(rgr.len(), 64);
            rgr.release();
        });
    }

    #[test]
    fn async_frame_undercommit() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<512>, 1> = PubSubChannel::new_static();
            let mut publisher = pubsub.framed_publisher().unwrap();
            let mut subscriber = pubsub.framed_subscriber().unwrap();

            for _ in 0..100 {
                // Create largeish grants
                let mut wgr = publisher.grant_async(128).await.unwrap();
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = i as u8;
                }
                wgr.commit(13);

                let mut wgr = publisher.grant_async(64).await.unwrap();
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = (i as u8) + 128;
                }
                wgr.commit(7);

                let mut wgr = publisher.grant_async(32).await.unwrap();
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = (i as u8) + 192;
                }
                wgr.commit(0);

                let rgr = subscriber.read_async().await.unwrap();
                assert_eq!(rgr.len(), 13);
                rgr.release();

                let rgr = subscriber.read_async().await.unwrap();
                assert_eq!(rgr.len(), 7);
                rgr.release();

                let rgr = subscriber.read_async().await.unwrap();
                assert_eq!(rgr.len(), 0);
                rgr.release();
            }
        });
    }

    #[test]
    fn async_frame_auto_commit_release() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> = PubSubChannel::new_static();
            let mut publisher = pubsub.framed_publisher().unwrap();
            let mut subscriber = pubsub.framed_subscriber().unwrap();

            for _ in 0..100 {
                {
                    let mut wgr = publisher.grant_async(64).await.unwrap();
                    wgr.to_commit(64);
                    for (i, by) in wgr.iter_mut().enumerate() {
                        *by = i as u8;
                    }
                    // drop
                }

                {
                    let mut rgr = subscriber.read_async().await.unwrap();
                    rgr.auto_release(true);
                    let rgr = rgr;

                    for (i, by) in rgr.iter().enumerate() {
                        assert_eq!(*by, i as u8);
                    }
                    assert_eq!(rgr.len(), 64);
                    // drop
                }
            }

            assert!(subscriber.read().is_none());
        });
    }
}
