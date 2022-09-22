use fugit::TimerInstantU32;
use std::{
    convert::Infallible,
    time::{SystemTime, UNIX_EPOCH},
};
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

impl fugit_timer::Timer<1000> for SysClock {
    type Error = Infallible;

    fn now(&mut self) -> fugit::TimerInstantU32<1000> {
        TimerInstantU32::from_ticks(SysClock::now(self))
    }

    fn start(&mut self, duration: fugit::TimerDurationU32<1000>) -> Result<(), Self::Error> {
        todo!()
    }

    fn cancel(&mut self) -> Result<(), Self::Error> {
        todo!()
    }

    fn wait(&mut self) -> nb::Result<(), Self::Error> {
        todo!()
    }
}
