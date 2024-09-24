use bitmaps::{Bits, BitsImpl};

/// The header of message in the PubSub queue will look like:
///
/// ```text
/// |  FrameHeader  |  SubsHeader   |   Payload     |
/// |  1..=9 bytes  |  1..=9 bytes  | 0..2^64 bytes |
/// ```
///
/// Where:
/// - the `FrameHeader` contains a variable length encoded length of the `SubsHeader` + `Payload`
/// - the `SubsHeader` contains a map of subscribers, with one bit per subscriber in the const generic `SUBS`, denoting whether or not that particular subscriber has read the message yet
/// - the `Payload` contains the raw MQTT message
///
/// The first byte of the `SubsHeader` is a bitstruct with the format:
/// ```text
/// b0..b4  (LSB): Number of bytes following the first byte in the `SubsHeader`
/// b4..b8       : Dont care
/// ```
pub struct Header;

impl Header {
    pub fn subs_header_len<const SUBS: usize>() -> usize
    where
        BitsImpl<SUBS>: Bits,
    {
        if SUBS > 1 {
            bitmaps::Bitmap::<SUBS>::new().as_bytes().len()
        } else {
            0
        }
    }

    pub fn encoded_len(value: usize) -> usize {
        vusize::encoded_len(value)
    }

    pub fn encode_usize_to_slice(value: usize, length: usize, slice: &mut [u8]) {
        vusize::encode_usize_to_slice(value, length, slice)
    }

    pub fn decoded_len(byte: u8) -> usize {
        vusize::decoded_len(byte)
    }

    pub fn decode_usize(input: &[u8]) -> usize {
        vusize::decode_usize(input)
    }
}

mod vusize {
    //! Varints
    //!
    //! This implementation borrows heavily from the `vint64` crate.
    //!
    //! Below is an example of how prefix bits signal the length of the integer value
    //! which follows:
    //!
    //! | Prefix     | Precision | Total Bytes |
    //! |------------|-----------|-------------|
    //! | `xxxxxxx1` | 7 bits    | 1 byte      |
    //! | `xxxxxx10` | 14 bits   | 2 bytes     |
    //! | `xxxxx100` | 21 bits   | 3 bytes     |
    //! | `xxxx1000` | 28 bits   | 4 bytes     |
    //! | `xxx10000` | 35 bits   | 5 bytes     |
    //! | `xx100000` | 42 bits   | 6 bytes     |
    //! | `x1000000` | 49 bits   | 7 bytes     |
    //! | `10000000` | 56 bits   | 8 bytes     |
    //! | `00000000` | 64 bits   | 9 bytes     |

    const USIZE_SIZE: usize = core::mem::size_of::<usize>();
    const USIZE_SIZE_PLUS_ONE: usize = USIZE_SIZE + 1;

    const fn max_size_header() -> u8 {
        // 64-bit: 0b0000_0000
        // 32-bit: 0b0001_0000
        // 16-bit: 0b0000_0100
        //  8-bit: 0b0000_0010
        ((1usize << USIZE_SIZE) & 0xFF) as u8
    }

    /// Get the length of an encoded `usize` for the given value in bytes.
    #[cfg(target_pointer_width = "64")]
    pub fn encoded_len(value: usize) -> usize {
        match value.leading_zeros() {
            0..=7 => 9,
            8..=14 => 8,
            15..=21 => 7,
            22..=28 => 6,
            29..=35 => 5,
            36..=42 => 4,
            43..=49 => 3,
            50..=56 => 2,
            57..=64 => 1,
            _ => {
                // SAFETY:
                //
                // The `leading_zeros` intrinsic returns the number of bits that
                // contain a zero bit. The result will always be in the range of
                // 0..=64 for a 64 bit `usize`, so the above pattern is exhaustive, however
                // it is not exhaustive over the return type of `u32`. Because of
                // this, we mark the "uncovered" part of the match as unreachable
                // for performance reasons.
                #[allow(unsafe_code)]
                unsafe {
                    core::hint::unreachable_unchecked()
                }
            }
        }
    }

    /// Get the length of an encoded `usize` for the given value in bytes.
    #[cfg(target_pointer_width = "32")]
    pub fn encoded_len(value: usize) -> usize {
        match value.leading_zeros() {
            0..=3 => 5,
            4..=10 => 4,
            11..=17 => 3,
            18..=24 => 2,
            25..=32 => 1,
            _ => {
                // SAFETY:
                //
                // The `leading_zeros` intrinsic returns the number of bits that
                // contain a zero bit. The result will always be in the range of
                // 0..=32 for a 32 bit `usize`, so the above pattern is exhaustive, however
                // it is not exhaustive over the return type of `u32`. Because of
                // this, we mark the "uncovered" part of the match as unreachable
                // for performance reasons.
                #[allow(unsafe_code)]
                unsafe {
                    core::hint::unreachable_unchecked()
                }
            }
        }
    }

    /// Get the length of an encoded `usize` for the given value in bytes.
    #[cfg(target_pointer_width = "16")]
    pub fn encoded_len(value: usize) -> usize {
        match value.leading_zeros() {
            0..=1 => 3,
            2..=8 => 2,
            9..=16 => 1,
            _ => {
                // SAFETY:
                //
                // The `leading_zeros` intrinsic returns the number of bits that
                // contain a zero bit. The result will always be in the range of
                // 0..=16 for a 16 bit `usize`, so the above pattern is exhaustive, however
                // it is not exhaustive over the return type of `u32`. Because of
                // this, we mark the "uncovered" part of the match as unreachable
                // for performance reasons.
                #[allow(unsafe_code)]
                unsafe {
                    core::hint::unreachable_unchecked()
                }
            }
        }
    }

    /// Get the length of an encoded `usize` for the given value in bytes.
    #[cfg(not(any(
        target_pointer_width = "16",
        target_pointer_width = "32",
        target_pointer_width = "64"
    )))]
    pub fn encoded_len(value: usize) -> usize {
        // I don't think you can have targets with 8 bit pointers in rust,
        // but just in case, 0..=127 would fit in one byte, and 128..=255
        // would fit in two.
        if (value & 0x80) == 0x80 {
            2
        } else {
            1
        }
    }

    /// Encode the given usize to the `slice`, using `length` bytes for encoding.
    ///
    /// ## Safety
    ///
    /// * `slice.len()` must be >= `length` or this function will panic
    /// * `length` must be `>= encoded_len(value)` or the value will be truncated
    /// * `length` must be `<= size_of::<usize>() + 1` or the value will be truncated
    pub fn encode_usize_to_slice(value: usize, length: usize, slice: &mut [u8]) {
        debug_assert!(
            encoded_len(value) <= length,
            "Tried to encode to smaller than necessary length!",
        );
        debug_assert!(length <= slice.len(), "Not enough space to encode!",);
        debug_assert!(
            length <= USIZE_SIZE_PLUS_ONE,
            "Tried to encode larger than platform supports!",
        );

        let header_bytes = &mut slice[..length];

        if length >= USIZE_SIZE_PLUS_ONE {
            // In the case where the number of bytes is larger than `usize`,
            // don't try to encode bits in the header byte, just create the header
            // and place all of the length bytes in subsequent bytes
            header_bytes[0] = max_size_header();
            header_bytes[1..USIZE_SIZE_PLUS_ONE].copy_from_slice(&value.to_le_bytes());
        } else {
            let encoded = (value << 1 | 1) << (length - 1);
            header_bytes.copy_from_slice(&encoded.to_le_bytes()[..length]);
        }
    }

    /// Determine the size of the encoded value (in bytes) based on the
    /// encoded header
    pub fn decoded_len(byte: u8) -> usize {
        byte.trailing_zeros() as usize + 1
    }

    /// Decode an encoded usize.
    ///
    /// Accepts a slice containing the encoded usize.
    pub fn decode_usize(input: &[u8]) -> usize {
        let length = decoded_len(input[0]);

        debug_assert!(input.len() >= length, "Not enough data to decode!",);
        debug_assert!(
            length <= USIZE_SIZE_PLUS_ONE,
            "Tried to decode data too large for this platform!",
        );

        let header_bytes = &input[..length];

        let mut encoded = [0u8; USIZE_SIZE];

        if length >= USIZE_SIZE_PLUS_ONE {
            // usize + 1 special case, see `encode_usize_to_slice()` for details
            encoded.copy_from_slice(&header_bytes[1..]);
            usize::from_le_bytes(encoded)
        } else {
            encoded[..length].copy_from_slice(header_bytes);
            usize::from_le_bytes(encoded) >> length
        }
    }
}
