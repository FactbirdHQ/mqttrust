use embedded_hal::timer::CountDown;
use std::time::{Duration, Instant};

pub struct SysTimer {
    start: Instant,
    count: u32,
}

impl SysTimer {
    pub fn new() -> SysTimer {
        SysTimer {
            start: Instant::now(),
            count: 0,
        }
    }
}

impl CountDown for SysTimer {
    type Error = core::convert::Infallible;
    type Time = u32;
    fn try_start<T>(&mut self, count: T) -> Result<(), Self::Error>
    where
        T: Into<Self::Time>,
    {
        self.start = Instant::now();
        self.count = count.into();
        Ok(())
    }
    fn try_wait(&mut self) -> nb::Result<(), Self::Error> {
        if Instant::now() - self.start > Duration::from_millis(self.count as u64) {
            Ok(())
        } else {
            Err(nb::Error::WouldBlock)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate nb;
}
