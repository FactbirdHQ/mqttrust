mod publisher;
mod state;
mod subscriber;

use core::cell::RefCell;

use embassy_sync::blocking_mutex::{raw::RawMutex, Mutex};
pub use publisher::*;
pub use subscriber::*;

mod buffer_provider;
pub use buffer_provider::*;

use crate::pubsub::vusize::{decode_usize, decoded_len};

use self::framed::{FrameGrantR, FramePublisher, FrameSubscriber};

pub mod framed;
pub(crate) mod vusize;

/// Result type used by the `BBQueue` interfaces
pub type Result<T> = core::result::Result<T, Error>;

/// Error type used by the `BBQueue` interfaces
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Error {
    /// The buffer does not contain sufficient size for the requested action
    InsufficientSize,

    /// Unable to produce another grant, a grant of this type is already in
    /// progress
    GrantInProgress,

    PublisherAlreadyTaken,

    MaximumSubscribersReached,
}

pub struct PubSubChannel<M: RawMutex, B: BufferProvider, const SUBS: usize> {
    inner: Mutex<M, RefCell<state::PubSubState<B, SUBS>>>,
}

impl<M: RawMutex, B: BufferProvider, const SUBS: usize> PubSubChannel<M, B, SUBS> {
    pub fn new(buf: B) -> Self {
        Self {
            inner: Mutex::const_new(M::INIT, RefCell::new(state::PubSubState::new(buf))),
        }
    }

    pub fn read(&mut self) -> Result<GrantR<'_, M, B, SUBS>> {
        self.inner.lock(|state| {
            let mut inner = state.borrow_mut();

            if inner.read_in_progress {
                return Err(Error::GrantInProgress);
            }
            inner.read_in_progress = true;

            let mut read = inner.read;

            // Resolve the inverted case or end of read
            if (read == inner.last) && (inner.write < read) {
                read = 0;
                // This has some room for error, the other thread reads this
                // Impact to Grant:
                //   Grant checks if read < write to see if inverted. If not inverted, but
                //     no space left, Grant will initiate an inversion, but will not trigger it
                // Impact to Commit:
                //   Commit does not check read, but if Grant has started an inversion,
                //   grant could move Last to the prior write position
                // MOVING READ BACKWARDS!
                inner.read = 0;
            }

            let sz = if inner.write < read {
                // Inverted, only believe last
                inner.last
            } else {
                // Not inverted, only believe write
                inner.write
            } - read;

            if sz == 0 {
                inner.read_in_progress = false;
                return Err(Error::InsufficientSize);
            }

            // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
            // are all `#[repr(Transparent)]
            let start_of_buf_ptr =
                unsafe { (&mut *inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
            let grant_slice = unsafe {
                core::slice::from_raw_parts_mut(start_of_buf_ptr.offset(read as isize), sz)
            };

            Ok(GrantR {
                buf: grant_slice,
                channel: &self.inner,
                to_release: 0,
            })
        })
    }

    /// Obtain the next available frame, if any
    pub fn read_framed(&mut self) -> Option<FrameGrantR<'_, M, B, SUBS>> {
        // Get all available bytes. We never wrap a frame around,
        // so if a header is available, the whole frame will be.
        let mut grant_r = self.read().ok()?;

        // Additionally, we never commit less than a full frame with
        // a header, so if we have ANY data, we'll have a full header
        // and frame. `Subscriber::read` will return an Error when
        // there are 0 bytes available.

        // The header consists of a single usize, encoded in native
        // endianess order
        let frame_len = decode_usize(&grant_r);
        let hdr_len = decoded_len(grant_r[0]);
        let total_len = frame_len + hdr_len;
        let hdr_len = hdr_len as u8;

        debug_assert!(grant_r.len() >= total_len);

        // Reduce the grant down to the size of the frame with a header
        grant_r.shrink(total_len);

        Some(FrameGrantR { grant_r, hdr_len })
    }

    /// Create a new `Subscriber`. It will only receive messages that are published after its creation.
    ///
    /// If there are no subscriber slots left, an error will be returned.
    pub fn subscriber(&self) -> Result<Subscriber<'_, M, B, SUBS>> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.subscriber_count >= SUBS {
                Err(Error::MaximumSubscribersReached)
            } else {
                s.subscriber_count += 1;
                Ok(Subscriber::new(&self.inner))
            }
        })
    }

    /// Create a new `FrameSubscriber`. It will only receive messages that are published after its creation.
    ///
    /// If there are no subscriber slots left, an error will be returned.
    pub fn framed_subscriber(&self) -> Result<FrameSubscriber<'_, M, B, SUBS>> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.subscriber_count >= SUBS {
                Err(Error::MaximumSubscribersReached)
            } else {
                s.subscriber_count += 1;
                Ok(FrameSubscriber::new(&self.inner))
            }
        })
    }

    /// Create a new `Publisher`.
    ///
    /// If a publisher has already been taken, an error will be returned.
    pub fn publisher(&self) -> Result<Publisher<'_, M, B, SUBS>> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.publisher_taken {
                Err(Error::PublisherAlreadyTaken)
            } else {
                s.publisher_taken = true;
                Ok(Publisher::new(&self.inner))
            }
        })
    }

    /// Create a new `Publisher`.
    ///
    /// If a publisher has already been taken, an error will be returned.
    pub fn framed_publisher(&self) -> Result<FramePublisher<'_, M, B, SUBS>> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.publisher_taken {
                Err(Error::PublisherAlreadyTaken)
            } else {
                s.publisher_taken = true;
                Ok(FramePublisher::new(&self.inner))
            }
        })
    }
}

impl<'a, M: RawMutex, const SUBS: usize> PubSubChannel<M, SliceBufferProvider<'a>, SUBS> {
    pub fn new_from_slice(buf: &'a mut [u8]) -> Self {
        Self::new(SliceBufferProvider::new(buf))
    }
}

impl<'a, M: RawMutex, const N: usize, const SUBS: usize>
    PubSubChannel<M, StaticBufferProvider<N>, SUBS>
{
    pub const fn new_static() -> Self {
        Self {
            inner: Mutex::const_new(M::INIT, RefCell::new(state::PubSubState::new_static())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, PubSubChannel, StaticBufferProvider};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use rand::prelude::*;
    use std::thread::spawn;
    use std::time::{Duration, Instant};

    const ITERS: usize = 10_000;

    const RPT_IVAL: usize = ITERS / 100;

    const QUEUE_SIZE: usize = 1024;

    const TIMEOUT_NODATA: Duration = Duration::from_millis(10_000);

    #[test]
    fn randomize_tx() {
        println!("RTX: Generating Test Data...");
        let gen_start = Instant::now();
        let mut data = Vec::with_capacity(ITERS);
        (0..ITERS).for_each(|_| data.push(rand::random::<u8>()));
        let mut data_rx = data.clone();

        let mut trng = thread_rng();
        let mut chunks = vec![];
        while !data.is_empty() {
            let chunk_sz = trng.gen_range(1..(1024 - 1) / 2);
            if chunk_sz > data.len() {
                continue;
            }

            // Note: This gives back data in chunks in reverse order.
            // We later .rev()` this to fix it
            chunks.push(data.split_off(data.len() - chunk_sz));
        }

        println!("RTX: Generation complete: {:?}", gen_start.elapsed());

        println!("RTX: Running test...");

        static mut PUBSUB: PubSubChannel<
            CriticalSectionRawMutex,
            StaticBufferProvider<QUEUE_SIZE>,
            1,
        > = PubSubChannel::new_static();
        let mut tx = unsafe { PUBSUB.publisher().unwrap() };
        let mut rx = unsafe { PUBSUB.subscriber().unwrap() };

        let mut last_tx = Instant::now();
        let mut last_rx = last_tx.clone();
        let start_time = last_tx.clone();

        let tx_thr = spawn(move || {
            let mut txd_ct = 0;
            let mut txd_ivl = 0;

            for (i, ch) in chunks.iter().rev().enumerate() {
                let mut semichunk = ch.to_owned();
                // println!("semi: {:?}", semichunk);

                while !semichunk.is_empty() {
                    if last_tx.elapsed() > TIMEOUT_NODATA {
                        panic!("tx timeout, iter {}", i);
                    }

                    'sizer: for sz in (1..(semichunk.len() + 1)).rev() {
                        if let Ok(mut gr) = tx.grant_exact(sz) {
                            // how do you do this idiomatically?
                            (0..sz).for_each(|idx| {
                                gr[idx] = semichunk.remove(0);
                            });
                            gr.commit(sz);

                            // Update tracking
                            last_tx = Instant::now();
                            txd_ct += sz;
                            if (txd_ct / RPT_IVAL) > txd_ivl {
                                txd_ivl = txd_ct / RPT_IVAL;

                                println!("{:?} - rtxtx: {}", start_time.elapsed(), txd_ct);
                            }

                            break 'sizer;
                        }
                    }
                }
            }
        });

        let rx_thr = spawn(move || {
            let mut rxd_ct = 0;
            let mut rxd_ivl = 0;

            for (_idx, i) in data_rx.drain(..).enumerate() {
                'inner: loop {
                    if last_rx.elapsed() > TIMEOUT_NODATA {
                        panic!("rx timeout, iter {}", i);
                    }
                    let gr = match rx.read() {
                        Ok(gr) => gr,
                        Err(Error::InsufficientSize) => continue 'inner,
                        Err(_) => panic!(),
                    };

                    let act = gr[0] as u8;
                    let exp = i;
                    if act != exp {
                        println!("act: {:?}, exp: {:?}", act, exp);

                        println!("len: {:?}", gr.len());

                        // println!("{:?}", gr);
                        panic!("RX Iter: {}, mod: {}", i, i % 6);
                    }
                    gr.release(1);

                    // Update tracking
                    last_rx = Instant::now();
                    rxd_ct += 1;
                    if (rxd_ct / RPT_IVAL) > rxd_ivl {
                        rxd_ivl = rxd_ct / RPT_IVAL;

                        println!("{:?} - rtxrx: {}", start_time.elapsed(), rxd_ct);
                    }

                    break 'inner;
                }
            }
        });

        tx_thr.join().unwrap();
        rx_thr.join().unwrap();
    }

    #[test]
    fn sanity_check() {
        static mut PUBSUB: PubSubChannel<
            CriticalSectionRawMutex,
            StaticBufferProvider<QUEUE_SIZE>,
            1,
        > = PubSubChannel::new_static();
        let mut tx = unsafe { PUBSUB.publisher().unwrap() };
        let mut rx = unsafe { PUBSUB.subscriber().unwrap() };

        let mut last_tx = Instant::now();
        let mut last_rx = last_tx.clone();
        let start_time = last_tx.clone();

        let tx_thr = spawn(move || {
            let mut txd_ct = 0;
            let mut txd_ivl = 0;

            for i in 0..ITERS {
                'inner: loop {
                    if last_tx.elapsed() > TIMEOUT_NODATA {
                        panic!("tx timeout, iter {}", i);
                    }
                    match tx.grant_exact(1) {
                        Ok(mut gr) => {
                            gr[0] = (i & 0xFF) as u8;
                            gr.commit(1);

                            // Update tracking
                            last_tx = Instant::now();
                            txd_ct += 1;
                            if (txd_ct / RPT_IVAL) > txd_ivl {
                                txd_ivl = txd_ct / RPT_IVAL;

                                println!("{:?} - sctx: {}", start_time.elapsed(), txd_ct);
                            }

                            break 'inner;
                        }
                        Err(_) => {}
                    }
                }
            }
        });

        let rx_thr = spawn(move || {
            let mut rxd_ct = 0;
            let mut rxd_ivl = 0;

            let mut i = 0;

            while i < ITERS {
                if last_rx.elapsed() > TIMEOUT_NODATA {
                    panic!("rx timeout, iter {}", i);
                }

                let gr = match rx.read() {
                    Ok(gr) => gr,
                    Err(Error::InsufficientSize) => continue,
                    Err(_) => panic!(),
                };

                for data in &*gr {
                    let act = *data;
                    let exp = (i & 0xFF) as u8;
                    if act != exp {
                        // println!("baseptr: {}", panny);

                        println!("offendr: {:p}", &gr[0]);

                        println!("act: {:?}, exp: {:?}", act, exp);

                        println!("len: {:?}", gr.len());

                        // println!("{:?}", &gr);
                        panic!("RX Iter: {}, mod: {}", i, i % 6);
                    }

                    i += 1;
                }

                let len = gr.len();
                rxd_ct += len;
                gr.release(len);

                // Update tracking
                last_rx = Instant::now();
                if (rxd_ct / RPT_IVAL) > rxd_ivl {
                    rxd_ivl = rxd_ct / RPT_IVAL;

                    println!("{:?} - scrx: {}", start_time.elapsed(), rxd_ct);
                }
            }
        });

        tx_thr.join().unwrap();
        rx_thr.join().unwrap();
    }

    #[test]
    fn sanity_check_grant_max() {
        static mut PUBSUB: PubSubChannel<
            CriticalSectionRawMutex,
            StaticBufferProvider<QUEUE_SIZE>,
            1,
        > = PubSubChannel::new_static();
        let mut tx = unsafe { PUBSUB.publisher().unwrap() };
        let mut rx = unsafe { PUBSUB.subscriber().unwrap() };

        println!("SCGM: Generating Test Data...");
        let gen_start = Instant::now();

        let mut data_tx = (0..ITERS).map(|i| (i & 0xFF) as u8).collect::<Vec<_>>();
        let mut data_rx = data_tx.clone();

        println!("SCGM: Generated Test Data in: {:?}", gen_start.elapsed());

        println!("SCGM: Starting Test...");

        let mut last_tx = Instant::now();
        let mut last_rx = last_tx.clone();
        let start_time = last_tx.clone();

        let tx_thr = spawn(move || {
            let mut txd_ct = 0;
            let mut txd_ivl = 0;

            let mut trng = thread_rng();

            while !data_tx.is_empty() {
                'inner: loop {
                    if last_tx.elapsed() > TIMEOUT_NODATA {
                        panic!("tx timeout");
                    }
                    match tx.grant_max_remaining(
                        trng.gen_range((QUEUE_SIZE / 3)..((2 * QUEUE_SIZE) / 3)),
                    ) {
                        Ok(mut gr) => {
                            let sz = ::std::cmp::min(data_tx.len(), gr.len());
                            for i in 0..sz {
                                gr[i] = data_tx.pop().unwrap();
                            }

                            // Update tracking
                            last_tx = Instant::now();
                            txd_ct += sz;
                            if (txd_ct / RPT_IVAL) > txd_ivl {
                                txd_ivl = txd_ct / RPT_IVAL;

                                println!("{:?} - scgmtx: {}", start_time.elapsed(), txd_ct);
                            }

                            let len = gr.len();
                            gr.commit(len);
                            break 'inner;
                        }
                        Err(_) => {}
                    }
                }
            }
        });

        let rx_thr = spawn(move || {
            let mut rxd_ct = 0;
            let mut rxd_ivl = 0;

            while !data_rx.is_empty() {
                'inner: loop {
                    if last_rx.elapsed() > TIMEOUT_NODATA {
                        panic!("rx timeout");
                    }
                    let gr = match rx.read() {
                        Ok(gr) => gr,
                        Err(Error::InsufficientSize) => continue 'inner,
                        Err(_) => panic!(),
                    };

                    let act = gr[0];
                    let exp = data_rx.pop().unwrap();
                    if act != exp {
                        println!("offendr: {:p}", &gr[0]);

                        println!("act: {:?}, exp: {:?}", act, exp);

                        println!("len: {:?}", gr.len());

                        // println!("{:?}", gr);
                        panic!("RX Iter: {}", rxd_ct);
                    }
                    gr.release(1);

                    // Update tracking
                    last_rx = Instant::now();
                    rxd_ct += 1;
                    if (rxd_ct / RPT_IVAL) > rxd_ivl {
                        rxd_ivl = rxd_ct / RPT_IVAL;

                        println!("{:?} - scgmrx: {}", start_time.elapsed(), rxd_ct);
                    }

                    break 'inner;
                }
            }
        });

        tx_thr.join().unwrap();
        rx_thr.join().unwrap();
    }
}
