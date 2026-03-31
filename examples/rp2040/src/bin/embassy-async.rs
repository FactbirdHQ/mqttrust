#![no_std]
#![no_main]

use core::net::Ipv4Addr;
use cyw43::{Aligned, JoinOptions, A4};
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};
use embassy_executor::Spawner;
use embassy_net::{
    tcp::client::{TcpClient, TcpClientState},
    StackResources,
};
use embassy_rp::{
    bind_interrupts,
    clocks::RoscRng,
    dma,
    gpio::{Level, Output},
    peripherals::{DMA_CH0, PIO0},
    pio::{InterruptHandler, Pio},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use mqttrust::{
    transport::embedded_nal::NalTransport, Config, IpBroker, MqttClient, MqttStack, Publish, State,
    Subscribe, SubscribeTopic,
};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
});

const WIFI_NETWORK: &str = "ssid"; // change to your network SSID
const WIFI_PASSWORD: &str = "pwd"; // change to your network password

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO0, 0>>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn mqtt_task(
    mut mqtt_stack: MqttStack<'static, NoopRawMutex>,
    broker: IpBroker,
    stack: embassy_net::Stack<'static>,
) -> ! {
    let client_state = TcpClientState::<1, 1024, 1024>::new();
    let tcp_client = TcpClient::new(stack, &client_state);

    let mut nal_transport = NalTransport::new(&tcp_client, broker);

    mqtt_stack.run(&mut nal_transport).await;
    unreachable!()
}

#[embassy_executor::task(pool_size = 2)]
async fn mqtt_subscription(
    client: &'static MqttClient<'static, NoopRawMutex>,
    topic: &'static str,
) {
    // Use the MQTT client to subscribe
    let sub_topic: SubscribeTopic = topic.into();

    let mut subscription = client
        .subscribe::<1>(Subscribe::builder().topics(&[sub_topic]).build())
        .await
        .unwrap();
    while let Some(message) = subscription.next_message().await {
        if message.topic_name() == topic {
            client
                .publish(
                    Publish::builder()
                        .payload(topic.as_bytes())
                        .topic_name("RECV")
                        .build(),
                )
                .await
                .unwrap();
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::info!("Hello World!");

    let p = embassy_rp::init(Default::default());

    let mut rng = RoscRng;

    // let fw = cyw43::aligned_bytes!("../../../../cyw43-firmware/43439A0.bin");
    // let clm = include_bytes!("../../../../cyw43-firmware/43439A0_clm.bin");
    // let nvram = cyw43::aligned_bytes!("../../../../cyw43-firmware/43439A0.txt");
    // To make flashing faster for development, you may want to flash the firmwares independently
    // at hardcoded addresses, instead of baking them into the program with `include_bytes!`:
    //     probe-rs download 43439A0.bin --binary-format bin --chip RP2040 --base-address 0x10100000
    //     probe-rs download 43439A0_clm.bin --binary-format bin --chip RP2040 --base-address 0x10140000
    //     probe-rs download 43439A0.txt --binary-format bin --chip RP2040 --base-address 0x10150000
    // SAFETY: Addresses are 4-byte aligned and point to firmware flashed via probe-rs.
    // Aligned<A4, [u8]> has the same layout as [u8] with 4-byte alignment.
    let fw: &Aligned<A4, [u8]> = unsafe {
        core::mem::transmute(core::slice::from_raw_parts(0x10100000 as *const u8, 230321))
    };
    let clm = unsafe { core::slice::from_raw_parts(0x10140000 as *const u8, 4752) };
    let nvram: &Aligned<A4, [u8]> =
        unsafe { core::mem::transmute(core::slice::from_raw_parts(0x10150000 as *const u8, 1073)) };

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let dma = dma::Channel::new(p.DMA_CH0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        dma,
    );

    static CYW_STATE: StaticCell<cyw43::State> = StaticCell::new();
    let cyw_state = CYW_STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(cyw_state, pwr, spi, fw, nvram).await;
    spawner.spawn(cyw43_task(runner).unwrap());

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let config = embassy_net::Config::dhcpv4(Default::default());
    // Use static IP configuration instead of DHCP
    //let config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
    //    address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 69, 2), 24),
    //    dns_servers: Vec::new(),
    //    gateway: Some(Ipv4Address::new(192, 168, 69, 1)),
    //});

    // Generate random seed
    let seed = rng.next_u64();

    // Init network stack
    static RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        net_device,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    spawner.spawn(net_task(runner).unwrap());

    // Setup `mqttrust`

    let client_id = "MyClient";

    // Create the MQTT stack
    let broker = IpBroker::new(Ipv4Addr::new(0, 0, 0, 0), 1883);
    let config = Config::builder()
        .client_id(client_id.try_into().unwrap())
        .keepalive_interval(embassy_time::Duration::from_secs(50))
        .build();

    static STATE: StaticCell<State<NoopRawMutex, 1024, 1024>> = StaticCell::new();
    let state = STATE.init(State::new());
    let (mqtt_stack, client) = mqttrust::new(state, config);

    static CLIENT: StaticCell<MqttClient<'static, NoopRawMutex>> = StaticCell::new();
    let client = CLIENT.init(client);

    loop {
        match control
            .join(WIFI_NETWORK, JoinOptions::new(WIFI_PASSWORD.as_bytes()))
            .await
        {
            Ok(_) => break,
            Err(err) => {
                defmt::info!("join failed with status={}", err);
            }
        }
    }

    // Wait for DHCP, not necessary when using static IP
    defmt::info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    defmt::info!("DHCP is now up!");

    spawner.spawn(mqtt_task(mqtt_stack, broker, stack).unwrap());
    spawner.spawn(mqtt_subscription(client, "ABC").unwrap());
    spawner.spawn(mqtt_subscription(client, "DEF").unwrap());

    loop {
        Timer::after(Duration::from_secs(2)).await;
        client
            .publish(Publish::builder().payload(b"").topic_name("ABC").build())
            .await
            .unwrap();

        Timer::after(Duration::from_secs(5)).await;
        client
            .publish(Publish::builder().payload(b"").topic_name("DEF").build())
            .await
            .unwrap();
    }
}
