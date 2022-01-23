use embedded_hal::timer::nb::{Cancel, CountDown};
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

impl CountDown for SysClock {
    type Error = ();

    type Time = u32;

    fn start<T>(&mut self, count: T) -> Result<(), Self::Error>
    where
        T: Into<Self::Time>,
    {
        let count_ms = count.into();
        self.countdown_end.replace(self.now() + count_ms);
        Ok(())
    }

    fn wait(&mut self) -> nb::Result<(), Self::Error> {
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
    fn cancel(&mut self) -> Result<(), Self::Error> {
        self.countdown_end.take();
        Ok(())
    }
}
