/// Trait for a buffer provider.
/// The Buffer provider allows abstraction over the memory
/// The memory can be statically allocated, on the heap or on the stack
pub trait BufferProvider: PartialEq {
    /// Returns a reference to the provided buffer
    /// The buffer **HAS NO GARANTEE** on it's state or initialization
    fn buf(&mut self) -> &mut [u8];
}

/// A statically allocated buffer
#[derive(Debug, PartialEq)]
pub struct StaticBufferProvider<const N: usize> {
    buf: [u8; N],
}

impl<const N: usize> StaticBufferProvider<N> {
    /// A buffer with internal allocation
    pub const fn new() -> Self {
        Self { buf: [0; N] }
    }
}

impl<const N: usize> BufferProvider for StaticBufferProvider<N> {
    fn buf(&mut self) -> &mut [u8] {
        &mut self.buf
    }
}

/// A buffer allocated from userspace
#[derive(Debug, PartialEq)]
pub struct SliceBufferProvider<'a> {
    buf: &'a mut [u8],
}

impl<'a> SliceBufferProvider<'a> {
    /// Creates a new BufferProvided from a userspace memory
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf }
    }
}

impl BufferProvider for SliceBufferProvider<'_> {
    fn buf(&mut self) -> &mut [u8] {
        &mut self.buf
    }
}
