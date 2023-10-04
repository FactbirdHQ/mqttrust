use core::ops::{Deref, DerefMut};

use super::{GrantR, GrantW};


/// A write grant for a single frame
///
/// NOTE: If the grant is dropped without explicitly commiting
/// the contents without first calling `to_commit()`, then no
/// frame will be comitted for writing.
#[derive(Debug, PartialEq)]
pub struct FrameGrantW<'a> {
    grant_w: GrantW<'a>,
    hdr_len: u8,
}

/// A read grant for a single frame
///
/// NOTE: If the grant is dropped without explicitly releasing
/// the contents, then no frame will be released.
#[derive(Debug, PartialEq)]
pub struct FrameGrantR<'a> {
    grant_r: GrantR<'a>,
    hdr_len: u8,
}

impl<'a> Deref for FrameGrantW<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.grant_w.buf[self.hdr_len.into()..]
    }
}

impl<'a> DerefMut for FrameGrantW<'a> {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.grant_w.buf[self.hdr_len.into()..]
    }
}

impl<'a> Deref for FrameGrantR<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.grant_r.buf[self.hdr_len.into()..]
    }
}

impl<'a> DerefMut for FrameGrantR<'a> {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.grant_r.buf[self.hdr_len.into()..]
    }
}

impl<'a> FrameGrantW<'a> {
    /// Commit a frame to make it available to the Consumer half.
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
        let frame_len = core::cmp::min(used, grant_len - hdr_len);
        let total_len = frame_len + hdr_len;

        // Write the actual frame length to the header
        encode_usize_to_slice(frame_len, hdr_len, &mut self.grant_w[..hdr_len]);

        total_len
    }

    /// Configures the amount of bytes to be commited on drop.
    pub fn to_commit(&mut self, amt: usize) {
        if amt == 0 {
            self.grant_w.to_commit(0);
        } else {
            let size = self.set_header(amt);
            self.grant_w.to_commit(size);
        }
    }
}

impl<'a> FrameGrantR<'a> {
    /// Release a frame to make the space available for future writing
    ///
    /// Note: The full frame is always released
    pub fn release(mut self) {
        // For a read grant, we have already shrunk the grant
        // size down to the correct size
        let len = self.grant_r.len();
        self.grant_r.release_inner(len);
    }

    /// Set whether the read fram should be automatically released
    pub fn auto_release(&mut self, is_auto: bool) {
        self.grant_r
            .to_release(if is_auto { self.grant_r.len() } else { 0 });
    }
}
