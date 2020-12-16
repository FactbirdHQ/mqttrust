#![no_main]
#![no_std]
use atat::{self, ClientBuilder, ComQueue, Queues, ResQueue, UrcQueue};
use core::cell::RefCell;
use core::slice::from_ref;
use cortex_m::peripheral::DWT;
use defmt_rtt as _; // global logger
use embedded_hal::blocking::delay::DelayMs;
use embedded_hal::timer::CountDown;
use embedded_nal::Ipv4Addr;
use hal::gpio::gpioc::PC4;
use hal::gpio::gpiod::PD11;
use hal::gpio::gpioe::PE8;
use hal::gpio::{OpenDrain, Output, PushPull};
use hal::pac::{TIM4, TIM6, TIM7, UART4};
use hal::prelude::*;
use hal::rcc::{MsiFreq, PllConfig, PllDivider, PllSource};
use hal::serial::{self, Event, Rx, Serial, Tx};
use hal::timer::Timer;
use heapless::consts;
use heapless::spsc::Queue;
use heapless::Vec;
use mqttrust::{MqttClient, MqttEvent, MqttOptions, Notification, Request};
use panic_semihosting as _;
use rtic::cyccnt::U32Ext;
use stm32l4xx_hal as hal;
use stm32l4xx_hal::time::Hertz;
use ublox_cellular::command::gpio::types::{GpioMode, GpioOutValue};
use ublox_cellular::command::gpio::SetGpioConfiguration;
use ublox_cellular::sockets::SocketSet;
use ublox_cellular::{APNInfo, Apn, Config as GSMConfig, ContextId, GsmClient, ProfileId};

const MQTT_CLIENT_ID: &str = "stm32l4xx_ubloxcellular";
const MQTT_BROKER_ADDR: [u8; 4] = [18, 198, 17, 154];
const MQTT_BROKER_PORT: u16 = 1883;

#[rtic::app(device = stm32l4xx_hal::pac, peripherals = true, monotonic = rtic::cyccnt::CYCCNT, dispatchers = [UART5])]
mod app {
    use super::*;

    #[resources]
    struct Resources {
        cell_client: RefCell<
            GsmClient<
                atat::Client<
                    Tx<UART4>,
                    TimerWrapper<Timer<TIM6>>,
                    consts::U1024,
                    consts::U3,
                    consts::U4,
                    consts::U10,
                >,
                TimerWrapper<Timer<TIM4>>,
                consts::U5,
                consts::U2048,
                PE8<Output<OpenDrain>>,
                PD11<Output<PushPull>>,
                PC4<Output<OpenDrain>>,
            >,
        >,
        cell_rx: Rx<UART4>,
        cell_ingress: atat::IngressManager<
            consts::U1024,
            atat::NoopUrcMatcher,
            consts::U3,
            consts::U4,
            consts::U10,
        >,
        mqtt_client: MqttClient<'static, 'static, consts::U10, Vec<u8, consts::U255>>,
        mqtt_event: MqttEvent<
            'static,
            'static,
            consts::U10,
            ublox_cellular::sockets::SocketHandle,
            TimerWrapper<Timer<TIM7>>,
            Vec<u8, consts::U255>,
        >,
    }

    #[init]
    fn init(mut ctx: init::Context) -> init::LateResources {
        static mut RES_QUEUE: ResQueue<consts::U1024, consts::U4> = Queue(heapless::i::Queue::u8());
        static mut URC_QUEUE: UrcQueue<consts::U1024, consts::U10> =
            Queue(heapless::i::Queue::u8());
        static mut COM_QUEUE: ComQueue<consts::U3> = Queue(heapless::i::Queue::u8());
        static mut MQTT_QUEUE: Queue<Request<Vec<u8, consts::U255>>, consts::U10, u8> =
            Queue(heapless::i::Queue::u8());
        static mut SOCKET_SET: Option<SocketSet<consts::U5, consts::U2048>> = None;

        // **           **
        // ** BEGIN BSP **
        // **           **
        // Enable the DWT monotonic cycle counter for RTFM scheduling
        ctx.core.DCB.enable_trace();
        DWT::unlock();
        ctx.core.DWT.enable_cycle_counter();

        // Declare peripherals
        let dp = ctx.device;

        // Set up the system clock.
        let mut flash = dp.FLASH.constrain();
        let mut rcc = dp.RCC.constrain();
        let mut pwr = dp.PWR.constrain(&mut rcc.apb1r1);

        let clocks = rcc
            .cfgr
            // System Clock source = PLL (MSI)
            .pll_source(PllSource::MSI)
            // MSI Frequency(Hz) = 4000000
            .msi(MsiFreq::RANGE4M)
            // SYSCLK(Hz) = 80,000,000, PLL_M = 1, PLL_N = 40, PLL_R = 2
            .sysclk_with_pll(80.mhz(), PllConfig::new(1, 40, PllDivider::Div2))
            .freeze(&mut flash.acr, &mut pwr);

        let cell_at_timer =
            TimerWrapper::new(Timer::tim6(dp.TIM6, 1000.hz(), clocks, &mut rcc.apb1r1));
        let cell_delay =
            TimerWrapper::new(Timer::tim4(dp.TIM4, 1000.hz(), clocks, &mut rcc.apb1r1));
        let mqtt_ping_timer =
            TimerWrapper::new(Timer::tim7(dp.TIM7, 1000.hz(), clocks, &mut rcc.apb1r1));

        let mut gpioa = dp.GPIOA.split(&mut rcc.ahb2);
        let mut gpiob = dp.GPIOB.split(&mut rcc.ahb2);
        let mut gpioc = dp.GPIOC.split(&mut rcc.ahb2);
        let mut gpioe = dp.GPIOE.split(&mut rcc.ahb2);
        let mut sim_sel = gpioa
            .pa4
            .into_push_pull_output(&mut gpioa.moder, &mut gpioa.otyper);

        sim_sel.try_set_high().ok();
        let cell_pwr = gpioc
            .pc4
            .into_open_drain_output(&mut gpioc.moder, &mut gpioc.otyper);
        let mut cell_nrst = gpioe
            .pe8
            .into_open_drain_output(&mut gpioe.moder, &mut gpioe.otyper);

        cell_nrst.try_set_high().ok();

        let mut cell_serial = {
            let tx = gpioa.pa0.into_af8(&mut gpioa.moder, &mut gpioa.afrl);
            let rx = gpioa.pa1.into_af8(&mut gpioa.moder, &mut gpioa.afrl);
            let rts = gpioa.pa15.into_af8(&mut gpioa.moder, &mut gpioa.afrh);
            let cts = gpiob.pb7.into_af8(&mut gpiob.moder, &mut gpiob.afrl);

            Serial::uart4(
                dp.UART4,
                (tx, rx, rts, cts),
                serial::Config::default()
                    .baudrate(230_400_u32.bps())
                    .character_match(b'\r')
                    .receiver_timeout(1000),
                clocks,
                &mut rcc.apb1r1,
            )
        };
        cell_serial.listen(Event::Rxne);

        // **           **
        // **  END BSP  **
        // **           **

        let (cell_tx, cell_rx) = cell_serial.split();
        let queues = Queues {
            res_queue: RES_QUEUE.split(),
            urc_queue: URC_QUEUE.split(),
            com_queue: COM_QUEUE.split(),
        };

        let (cell_at, cell_ingress) = ClientBuilder::new(
            cell_tx,
            cell_at_timer,
            atat::Config::new(atat::Mode::Timeout),
        )
        .build(queues);

        defmt::info!("Peripherals ready");

        let mut cell_client_inner = GsmClient::new(
            cell_at,
            cell_delay,
            GSMConfig::default()
                .with_rst(cell_nrst)
                .with_pwr(cell_pwr)
                .with_flow_control()
                .baud_rate(230_400_u32),
        );

        SOCKET_SET.replace(SocketSet::new());
        cell_client_inner.set_socket_storage(SOCKET_SET.as_mut().unwrap());
        let cell_client = RefCell::new(cell_client_inner);

        let (mqtt_producer, mqtt_consumer) = MQTT_QUEUE.split();
        let mqtt_client = MqttClient::new(mqtt_producer, MQTT_CLIENT_ID);
        let mqtt_event = MqttEvent::new(
            mqtt_consumer,
            mqtt_ping_timer,
            MqttOptions::new(
                MQTT_CLIENT_ID,
                Ipv4Addr::from(MQTT_BROKER_ADDR).into(),
                MQTT_BROKER_PORT,
            )
            .set_clean_session(true),
        );

        defmt::info!("Initialization step done");

        atat_spin::spawn().unwrap();

        init::LateResources {
            cell_client,
            cell_rx,
            cell_ingress,
            mqtt_client,
            mqtt_event,
        }
    }

    /// Idle thread - Captures the time the cpu is asleep to calculate cpu uasge
    #[idle(resources = [&cell_client, mqtt_event])]
    fn idle(mut ctx: idle::Context) -> ! {
        let apn_info = APNInfo {
            apn: Apn::Given(heapless::String::from("em")),
            ..APNInfo::default()
        };

        let mut init = false;
        let mut connected = false;

        loop {
            let mut cell_client = match ctx.resources.cell_client.try_borrow_mut() {
                Err(_) => {
                    defmt::error!("No cell client");
                    continue;
                }
                Ok(cell_client) => cell_client,
            };

            let data = match cell_client.data_service(ProfileId(0), ContextId(2), &apn_info) {
                Err(nb::Error::WouldBlock) => {
                    continue;
                }
                Err(_) => {
                    defmt::error!("No data service");
                    continue;
                }
                Ok(data) => data,
            };

            if !init {
                data.send_at(&SetGpioConfiguration {
                    gpio_id: 22,
                    gpio_mode: GpioMode::NetworkStatus,
                })
                .ok();

                data.send_at(&SetGpioConfiguration {
                    gpio_id: 21,
                    gpio_mode: GpioMode::Output(GpioOutValue::High),
                })
                .ok();

                defmt::info!("Initilaize done");
                init = true;
            }

            if !connected {
                match ctx
                    .resources
                    .mqtt_event
                    .lock(|mqtt_event| mqtt_event.connect(&data))
                {
                    Err(nb::Error::WouldBlock) => {
                        continue;
                    }
                    Err(_) => {
                        defmt::info!("MQTT connection error");
                        continue;
                    }
                    Ok(new_session) => {
                        if !new_session {
                        } else {
                            connected = true;
                        }
                    }
                }
            }

            if init || connected {
                match ctx
                    .resources
                    .mqtt_event
                    .lock(|mqtt_event| mqtt_event.yield_event(&data))
                {
                    Err(nb::Error::WouldBlock) => {
                        continue;
                    }
                    Err(_) => {
                        defmt::info!("MQTT yeild error");
                        unreachable!();
                    }
                    Ok(Notification::Abort(_)) => {
                        defmt::info!("MQTT yeild abort");
                        connected = false;
                    }
                    Ok(_ntf) => {
                        defmt::info!("MQTT notified");
                    }
                }
            }
        }
    }

    #[task(resources = [cell_ingress])]
    fn atat_spin(mut ctx: atat_spin::Context) {
        ctx.resources
            .cell_ingress
            .lock(|cell_ingress| cell_ingress.digest());
        atat_spin::schedule(ctx.scheduled + 1_000_000.cycles()).unwrap();
    }

    #[task(binds = UART4, resources = [cell_rx, cell_ingress])]
    fn uart4(mut ctx: uart4::Context) {
        nb::block!(ctx.resources.cell_rx.lock(|cell_rx| cell_rx.try_read()))
            .map(|word| {
                ctx.resources
                    .cell_ingress
                    .lock(|cell_ingress| cell_ingress.write(from_ref(&word)))
            })
            .ok();
    }

    pub struct Millis(u32);

    impl From<u32> for Millis {
        fn from(ms: u32) -> Self {
            Millis(ms)
        }
    }

    pub struct TimerWrapper<T>
    where
        T: CountDown<Time = Hertz>,
    {
        inner: T,
        remaining_ms: Millis,
    }

    impl<T> TimerWrapper<T>
    where
        T: CountDown<Time = Hertz>,
    {
        fn new(timer: T) -> Self {
            TimerWrapper {
                inner: timer,
                remaining_ms: Millis(0),
            }
        }
    }

    impl<T> DelayMs<u32> for TimerWrapper<T>
    where
        T: CountDown<Time = Hertz>,
    {
        type Error = T::Error;

        fn try_delay_ms(&mut self, ms: u32) -> Result<(), Self::Error> {
            self.try_start(ms)?;
            nb::block!(self.try_wait())
        }
    }

    impl<T> CountDown for TimerWrapper<T>
    where
        T: CountDown<Time = Hertz>,
    {
        type Error = T::Error;

        type Time = Millis;

        fn try_start<M>(&mut self, count: M) -> Result<(), Self::Error>
        where
            M: Into<Self::Time>,
        {
            let ms: Millis = count.into();

            self.inner
                .try_start::<Hertz>(core::cmp::min(ms.0, 1000).into())?;
            self.remaining_ms = Millis(ms.0.checked_sub(1000).unwrap_or_else(|| 0));
            Ok(())
        }

        fn try_wait(&mut self) -> nb::Result<(), Self::Error> {
            if self.remaining_ms.0 > 0 {
                self.inner.try_wait()?;
                self.inner
                    .try_start::<Hertz>(core::cmp::min(self.remaining_ms.0, 1000).into())?;
                self.remaining_ms =
                    Millis(self.remaining_ms.0.checked_sub(1000).unwrap_or_else(|| 0));
            }
            self.inner.try_wait()
        }
    }
}
