use embedded_time::clock::Error;
use embedded_time::fraction::Fraction;
use embedded_time::Clock;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SysClock {
    now: SystemTime,
}

impl SysClock {
    pub fn new() -> Self {
        Self {
            now: SystemTime::now(),
        }
    }
}

impl Clock for SysClock {
    const SCALING_FACTOR: Fraction = Fraction::new(1000, 1);
    type T = u32;

    fn try_now(&self) -> Result<embedded_time::Instant<Self>, Error> {
        Ok(embedded_time::Instant::new(
            self.now
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis() as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate nb;
}
