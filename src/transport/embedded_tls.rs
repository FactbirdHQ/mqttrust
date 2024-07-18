use core::{
    cell::{Cell, UnsafeCell},
    mem::MaybeUninit,
    ptr::NonNull,
};

use embedded_io_async::{Error as _, ErrorKind};
use embedded_nal_async::TcpConnect;
use embedded_tls::{CryptoProvider, TlsConfig, TlsContext, TlsError};

use super::Transport;

use crate::{error::ConnectionError, Broker, StateError};

pub struct TlsNalTransport<'a, N: TcpConnect, P, const RX: usize, const TX: usize> {
    network: &'a N,
    socket: Option<TlsConnection<'a, N, RX, TX>>,
    provider: P,
    tls_state: &'a TlsState<RX, TX>,
    tls_config: &'a TlsConfig<'a>,
}

impl<'a, N: TcpConnect, P, const RX: usize, const TX: usize> TlsNalTransport<'a, N, P, RX, TX> {
    pub fn new(
        network: &'a N,
        tls_state: &'a TlsState<RX, TX>,
        tls_config: &'a TlsConfig<'a>,
        provider: P,
    ) -> Self {
        Self {
            network,
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
        P: CryptoProvider<CipherSuite = embedded_tls::Aes128GcmSha256>,
        const RX: usize,
        const TX: usize,
    > Transport for TlsNalTransport<'a, N, P, RX, TX>
{
    type Socket = TlsConnection<'a, N, RX, TX>;

    async fn connect(&mut self, broker: &mut impl Broker) -> Result<(), ConnectionError> {
        let addr = broker
            .get_address()
            .await
            .ok_or(ConnectionError::InvalidAddress)?;

        let socket = self
            .network
            .connect(addr)
            .await
            .map_err(|e| ConnectionError::Io(e.kind()))?;

        debug!("Connected socket to {:?}", broker.get_hostname());

        let mut socket = TlsConnection::new(socket, self.tls_state).map_err(|e| {
            warn!("Failed tls connection: {:?}", e);
            ConnectionError::Io(e.kind())
        })?;

        socket
            .open(TlsContext::new(self.tls_config, &mut self.provider))
            .await
            .map_err(|e| ConnectionError::Io(e.kind()))?;

        debug!("Socket opened!");

        self.socket.replace(socket);

        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        self.socket.take();
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.socket.is_some()
    }

    fn socket(&mut self) -> Result<&mut Self::Socket, StateError> {
        self.socket
            .as_mut()
            .ok_or(StateError::Io(ErrorKind::NotConnected))
    }
}

pub struct TlsConnection<'d, N: TcpConnect + 'd, const RX: usize, const TX: usize> {
    socket: embedded_tls::TlsConnection<'d, N::Connection<'d>, embedded_tls::Aes128GcmSha256>,
    state: &'d TlsState<RX, TX>,
    bufs: NonNull<([u8; RX], [u8; TX])>,
}

impl<'d, N: TcpConnect, const RX: usize, const TX: usize> TlsConnection<'d, N, RX, TX> {
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

    pub async fn open<P: CryptoProvider<CipherSuite = embedded_tls::Aes128GcmSha256>>(
        &mut self,
        context: embedded_tls::TlsContext<'d, P>,
    ) -> Result<(), TlsError> {
        self.socket.open(context).await?;

        Ok(())
    }
}

impl<'d, N: TcpConnect, const RX: usize, const TX: usize> Drop for TlsConnection<'d, N, RX, TX> {
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
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.socket.read(buf).await
    }
}

impl<'d, N: TcpConnect, const RX: usize, const TX: usize> embedded_io_async::Write
    for TlsConnection<'d, N, RX, TX>
{
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.socket.write(buf).await
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.socket.flush().await
    }
}

/// State for TcpClient
pub struct TlsState<const RX: usize, const TX: usize> {
    pub(crate) pool: Pool<([u8; RX], [u8; TX]), 1>,
}

impl<const RX: usize, const TX: usize> Default for TlsState<RX, TX> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const RX: usize, const TX: usize> TlsState<RX, TX> {
    /// Create a new `TlsState`.
    pub const fn new() -> Self {
        Self { pool: Pool::new() }
    }
}

pub(crate) struct Pool<T, const N: usize> {
    used: [Cell<bool>; N],
    data: [UnsafeCell<MaybeUninit<T>>; N],
}

impl<T, const N: usize> Pool<T, N> {
    pub(crate) const fn new() -> Self {
        Self {
            used: [const { Cell::new(false) }; N],
            data: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
        }
    }
}

impl<T, const N: usize> Pool<T, N> {
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

    /// safety: p must be a pointer obtained from self.alloc that hasn't been freed yet.
    pub(crate) unsafe fn free(&self, p: NonNull<T>) {
        let origin = self.data.as_ptr() as *mut T;
        let n = p.as_ptr().offset_from(origin);
        assert!(n >= 0);
        assert!((n as usize) < N);
        self.used[n as usize].set(false);
    }
}
