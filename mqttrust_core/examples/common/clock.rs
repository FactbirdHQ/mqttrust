use embedded_hal::timer::{Cancel, CountDown};
use embedded_time::{clock::Error, rate::Fraction, Clock};
use std::time::{SystemTime, UNIX_EPOCH};
pub struct SysClock {
    start_time: u32,
    countdown_end: Option<u32>,
}

impl SysClock {
    pub fn new() -> Self {
        Self {
            start_time: Self::epoch(),
            countdown_end: None,
        }
    }

    pub fn epoch() -> u32 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis() as u32
    }

    pub fn now(&self) -> u32 {
        Self::epoch() - self.start_time
    }
}

impl Clock for SysClock {
    const SCALING_FACTOR: Fraction = Fraction::new(1, 1000);
    type T = u32;

    fn try_now(&self) -> Result<embedded_time::Instant<Self>, Error> {
        Ok(embedded_time::Instant::new(self.now()))
    }
}

impl CountDown for SysClock {
    type Error = ();

    type Time = u32;

    fn try_start<T>(&mut self, count: T) -> Result<(), Self::Error>
    where
        T: Into<Self::Time>,
    {
        let count_ms = count.into();
        self.countdown_end.replace(self.now() + count_ms);
        Ok(())
    }

    fn try_wait(&mut self) -> nb::Result<(), Self::Error> {
        match self.countdown_end.map(|end| end <= self.now()) {
            Some(true) => {
                self.countdown_end.take();
                Ok(())
            }
            Some(false) => Err(nb::Error::WouldBlock),
            None => Err(nb::Error::Other(())),
        }
    }
}

impl Cancel for SysClock {
    fn try_cancel(&mut self) -> Result<(), Self::Error> {
        self.countdown_end.take();
        Ok(())
    }
}
