use embedded_time::clock::Error;
use embedded_time::fraction::Fraction;
use embedded_time::Clock;
use std::time::Instant;

pub struct SysClock {
    now: Instant,
}

impl SysClock {
    pub fn new() -> Self {
        Self {
            now: Instant::now(),
        }
    }
}

impl Clock for SysClock {
    const SCALING_FACTOR: Fraction = Fraction::new(1000, 1);
    type T = u32;

    fn try_now(&self) -> Result<embedded_time::Instant<Self>, Error> {
        let _ = self.now;
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate nb;
}
