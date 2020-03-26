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
    type Time = u32;

    fn start<T>(&mut self, count: T)
    where
        T: Into<Self::Time>,
    {
        self.start = Instant::now();
        self.count = count.into();
    }

    fn wait(&mut self) -> nb::Result<(), void::Void> {
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
