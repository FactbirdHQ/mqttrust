#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use embedded_mqtt::exp::{MqttClient, MqttStack, State};
use embedded_mqtt::packets;
use embedded_mqtt::publication::Publication;
use embedded_mqtt::types::Properties;
use futures_util::StreamExt;
use static_cell::make_static;
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::task]
async fn mqtt_task(stack: MqttStack<'static, NoopRawMutex>) -> ! {
    stack.run().await
}

#[embassy_executor::task(pool_size = 2)]
async fn mqtt_subscription(
    client: &'static MqttClient<'static, NoopRawMutex>,
    topic: &'static str,
) {
    // Use the MQTT client to subscribe
    let subscribe = packets::Subscribe {
        packet_id: 16,
        properties: Properties::Slice(&[]),
        topics: &[topic.into()],
    };

    let mut subscription = client.subscribe(subscribe).await.unwrap();
    while let Some(message) = subscription.next().await {
        if message.topic() == topic {
            client
                .publish(
                    Publication::new(topic.as_bytes())
                        .topic("RECV")
                        .finish()
                        .unwrap(),
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

    // Create the MQTT stack
    let state = make_static!(State::<NoopRawMutex, 4096, 4096>::new());
    let (stack, client) = embedded_mqtt::exp::new(state);

    let client = make_static!(client);
    spawner.spawn(mqtt_task(stack)).unwrap();
    spawner.spawn(mqtt_subscription(client, "ABC")).unwrap();
    spawner.spawn(mqtt_subscription(client, "DEF")).unwrap();

    loop {
        Timer::after(Duration::from_secs(2)).await;
        client
            .publish(Publication::new(b"").topic("ABC").finish().unwrap())
            .await
            .unwrap();

        Timer::after(Duration::from_secs(5)).await;
        client
            .publish(Publication::new(b"").topic("DEF").finish().unwrap())
            .await
            .unwrap();
    }
}
