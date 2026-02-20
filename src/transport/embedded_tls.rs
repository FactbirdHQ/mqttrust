use core::{
    cell::{Cell, UnsafeCell},
    future::Future,
    mem::MaybeUninit,
    ptr::NonNull,
};

use embassy_time::Duration;
use embedded_io_async::{Error as _, ErrorKind};
use embedded_nal_async::TcpConnect;
use embedded_tls::{CryptoProvider, TlsConfig, TlsContext, TlsError};

use super::Transport;

use crate::{error::ConnectionError, Broker, StateError};

/// A transport layer for MQTT over TLS using a network abstraction layer (NAL).
///
/// This struct manages the connection to the MQTT broker using TLS for secure communication.
pub struct TlsNalTransport<'a, N: TcpConnect, B: Broker, P, const RX: usize, const TX: usize> {
    network: &'a N,
    broker: B,
    socket: Option<TlsConnection<'a, N, RX, TX>>,
    provider: P,
    tls_state: &'a TlsState<RX, TX>,
    tls_config: &'a TlsConfig<'a>,
}

impl<'a, N: TcpConnect, B: Broker, P, const RX: usize, const TX: usize>
    TlsNalTransport<'a, N, B, P, RX, TX>
{
    /// Creates a new `TlsNalTransport`.
    ///
    /// # Parameters
    ///
    /// - `network`: The network abstraction layer for TCP connections.
    /// - `broker`: The MQTT broker information.
    /// - `tls_state`: The TLS state for managing buffers.
    /// - `tls_config`: The TLS configuration.
    /// - `provider`: The cryptographic provider for TLS.
    ///
    /// # Returns
    ///
    /// A new instance of `TlsNalTransport`.
    pub fn new(
        network: &'a N,
        broker: B,
        tls_state: &'a TlsState<RX, TX>,
        tls_config: &'a TlsConfig<'a>,
        provider: P,
    ) -> Self {
        Self {
            network,
            broker,
            socket: None,
            provider,
            tls_config,
            tls_state,
        }
    }
}

impl<
        'a,
        N: TcpConnect,
        B: Broker,
        P: CryptoProvider<CipherSuite = embedded_tls::Aes128GcmSha256>,
        const RX: usize,
        const TX: usize,
    > Transport for TlsNalTransport<'a, N, B, P, RX, TX>
{
    type Socket = TlsConnection<'a, N, RX, TX>;

    /// Asynchronously connects to the MQTT broker using TLS.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the connection was successful, otherwise a `ConnectionError`.
    async fn connect(&mut self) -> Result<(), ConnectionError> {
        let addr = self
            .broker
            .get_address()
            .await
            .ok_or(ConnectionError::InvalidAddress)?;

        let socket = self
            .network
            .connect(addr)
            .await
            .map_err(|e| ConnectionError::Io(e.kind()))?;

        debug!("Connected socket to {:?}", self.broker.get_hostname());

        let mut socket = TlsConnection::new(socket, self.tls_state).map_err(|e| {
            warn!("Failed tls connection: {:?}", e);
            ConnectionError::Io(e.kind())
        })?;

        let open_fut = socket.open(TlsContext::new(self.tls_config, &mut self.provider));

        embassy_time::with_timeout(Duration::from_secs(5), open_fut)
            .await
            .map_err(|_| ConnectionError::NetworkTimeout)?
            .map_err(|e| ConnectionError::Io(e.kind()))?;

        debug!("Socket opened!");

        self.socket.replace(socket);

        Ok(())
    }

    /// Disconnects from the MQTT broker.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the disconnection was successful, otherwise a `ConnectionError`.
    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        self.socket.take();
        Ok(())
    }

    /// Checks if the transport is currently connected.
    ///
    /// # Returns
    ///
    /// `true` if the transport is connected, otherwise `false`.
    fn is_connected(&self) -> bool {
        self.socket.is_some()
    }

    /// Provides a mutable reference to the socket used by the transport.
    ///
    /// # Returns
    ///
    /// `Ok(&mut Self::Socket)` if the socket is available, otherwise a `StateError`.
    fn socket(&mut self) -> Result<&mut Self::Socket, StateError> {
        self.socket
            .as_mut()
            .ok_or(StateError::Io(ErrorKind::NotConnected))
    }
}

/// Represents a TLS connection for MQTT communication.
pub struct TlsConnection<'d, N: TcpConnect + 'd, const RX: usize, const TX: usize> {
    socket: embedded_tls::TlsConnection<'d, N::Connection<'d>, embedded_tls::Aes128GcmSha256>,
    state: &'d TlsState<RX, TX>,
    bufs: NonNull<([u8; RX], [u8; TX])>,
}

impl<'d, N: TcpConnect, const RX: usize, const TX: usize> TlsConnection<'d, N, RX, TX> {
    /// Creates a new `TlsConnection`.
    ///
    /// # Parameters
    ///
    /// - `socket`: The underlying TCP connection.
    /// - `state`: The TLS state for managing buffers.
    ///
    /// # Returns
    ///
    /// A new instance of `TlsConnection` or a `TlsError` if the connection could not be created.
    pub(crate) fn new(
        socket: N::Connection<'d>,
        state: &'d TlsState<RX, TX>,
    ) -> Result<Self, TlsError> {
        let mut bufs = state.pool.alloc().ok_or(TlsError::InsufficientSpace)?;

        let socket = unsafe {
            embedded_tls::TlsConnection::new(socket, &mut bufs.as_mut().0, &mut bufs.as_mut().1)
        };

        Ok(Self {
            socket,
            state,
            bufs,
        })
    }

    /// Opens the TLS connection.
    ///
    /// # Parameters
    ///
    /// - `context`: The TLS context containing the configuration and cryptographic provider.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the connection was opened successfully, otherwise a `TlsError`.
    pub fn open<'a, P: CryptoProvider<CipherSuite = embedded_tls::Aes128GcmSha256> + 'a>(
        &'a mut self,
        context: embedded_tls::TlsContext<'d, P>,
    ) -> impl Future<Output = Result<(), TlsError>> + 'a + use<'a, 'd, P, N, RX, TX> {
        self.socket.open(context)
    }
}

impl<'d, N: TcpConnect, const RX: usize, const TX: usize> Drop for TlsConnection<'d, N, RX, TX> {
    /// Frees the allocated buffers when the `TlsConnection` is dropped.
    fn drop(&mut self) {
        unsafe {
            self.state.pool.free(self.bufs);
        }
    }
}

impl<'d, N: TcpConnect, const RX: usize, const TX: usize> embedded_io_async::ErrorType
    for TlsConnection<'d, N, RX, TX>
{
    type Error = TlsError;
}

impl<'d, N: TcpConnect, const RX: usize, const TX: usize> embedded_io_async::Read
    for TlsConnection<'d, N, RX, TX>
{
    /// Reads data from the TLS connection.
    ///
    /// # Parameters
    ///
    /// - `buf`: The buffer to read data into.
    ///
    /// # Returns
    ///
    /// The number of bytes read or a `TlsError`.
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, Self::Error>> {
        self.socket.read(buf)
    }
}

impl<'d, N: TcpConnect, const RX: usize, const TX: usize> embedded_io_async::Write
    for TlsConnection<'d, N, RX, TX>
{
    /// Writes data to the TLS connection.
    ///
    /// # Parameters
    ///
    /// - `buf`: The buffer containing the data to write.
    ///
    /// # Returns
    ///
    /// The number of bytes written or a `TlsError`.
    fn write(&mut self, buf: &[u8]) -> impl Future<Output = Result<usize, Self::Error>> {
        self.socket.write(buf)
    }

    /// Flushes the TLS connection.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the flush was successful, otherwise a `TlsError`.
    fn flush(&mut self) -> impl Future<Output = Result<(), Self::Error>> {
        self.socket.flush()
    }
}

/// State for managing TLS buffers.
pub struct TlsState<const RX: usize, const TX: usize> {
    pub(crate) pool: Pool<([u8; RX], [u8; TX]), 1>,
}

impl<const RX: usize, const TX: usize> Default for TlsState<RX, TX> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const RX: usize, const TX: usize> TlsState<RX, TX> {
    /// Creates a new `TlsState`.
    ///
    /// # Returns
    ///
    /// A new instance of `TlsState`.
    pub const fn new() -> Self {
        Self { pool: Pool::new() }
    }
}

/// A memory pool for managing TLS buffers.
pub(crate) struct Pool<T, const N: usize> {
    used: [Cell<bool>; N],
    data: [UnsafeCell<MaybeUninit<T>>; N],
}

impl<T, const N: usize> Pool<T, N> {
    /// Creates a new `Pool`.
    ///
    /// # Returns
    ///
    /// A new instance of `Pool`.
    pub(crate) const fn new() -> Self {
        Self {
            used: [const { Cell::new(false) }; N],
            data: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
        }
    }
}

impl<T, const N: usize> Pool<T, N> {
    /// Allocates a buffer from the pool.
    ///
    /// # Returns
    ///
    /// A pointer to the allocated buffer or `None` if the pool is full.
    pub(crate) fn alloc(&self) -> Option<NonNull<T>> {
        for n in 0..N {
            // this can't race because Pool is not Sync.
            if !self.used[n].get() {
                self.used[n].set(true);
                let p = self.data[n].get() as *mut T;
                return Some(unsafe { NonNull::new_unchecked(p) });
            }
        }
        None
    }

    /// Frees a previously allocated buffer.
    ///
    /// # Safety
    ///
    /// The pointer `p` must be a pointer obtained from `self.alloc` that hasn't been freed yet.
    pub(crate) unsafe fn free(&self, p: NonNull<T>) {
        let origin = self.data.as_ptr() as *mut T;
        let n = p.as_ptr().offset_from(origin);
        assert!(n >= 0);
        assert!((n as usize) < N);
        self.used[n as usize].set(false);
    }
}
