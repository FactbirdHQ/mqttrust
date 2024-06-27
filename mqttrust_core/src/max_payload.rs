#[cfg(not(any(
    feature = "max_payload_size_2048",
    feature = "max_payload_size_4096",
    feature = "max_payload_size_8192"
)))]
pub const MAX_PAYLOAD_SIZE: usize = 4096;

#[cfg(feature = "max_payload_size_2048")]
pub const MAX_PAYLOAD_SIZE: usize = 2048;

#[cfg(feature = "max_payload_size_4096")]
pub const MAX_PAYLOAD_SIZE: usize = 4096;

#[cfg(feature = "max_payload_size_8192")]
pub const MAX_PAYLOAD_SIZE: usize = 8192;
