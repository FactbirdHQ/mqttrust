#![no_main]
use embedded_mqtt::pubsub::{PubSubChannel, StaticBufferProvider};
use futures::future::{join, join_all};
use libfuzzer_sys::fuzz_target;
use rand::Rng;
use std::sync::mpsc;

const MAX_SUBSCRIBERS: usize = 10;

fuzz_target!(|data: &[u8]| fuzz(data));

fn fuzz(data: &[u8]) {
    if std::env::var_os("RUST_LOG").is_some() {
        env_logger::init();
    }
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(fuzz_inner(data))
}

async fn fuzz_inner(data: &[u8]) {
    let pubsub: PubSubChannel<StaticBufferProvider<8192>, MAX_SUBSCRIBERS> =
        PubSubChannel::new(StaticBufferProvider::new());

    let (producer, consumer) = mpsc::channel();

    let num_subscribers = if !data.is_empty() {
        (data[0] % MAX_SUBSCRIBERS as u8) as usize + 1 // at least 1 subscriber
    } else {
        1
    };

    // Spawn subscribers
    let mut subscriber_handles = Vec::new();

    for _ in 0..num_subscribers {
        subscriber_handles.push(async {
            let mut subscriber = pubsub.subscriber().unwrap();
            let mut received_data = Vec::new();
            loop {
                match consumer.recv() {
                    Ok(true) => {
                        // Try to read a message
                        if let Ok(grant) = subscriber.read_any() {
                            received_data.extend_from_slice(grant.buf());
                            grant.release();
                        }
                    }
                    Ok(false) => {
                        // Signal to stop
                        break;
                    }
                    Err(_) => {
                        // Channel closed, stop
                        break;
                    }
                }
            }
            received_data
        });
    }

    // Producer thread
    let mut total_sent = 0;
    let producer_fut = async {
        let mut publisher = pubsub.publisher().unwrap();
        let mut rng = rand::thread_rng();
        while total_sent < data.len() {
            let remaining_data = &data[total_sent..];

            // Generate random data size to publish
            let max_grant_size = publisher.free_capacity() - 3; // Account for header
            let grant_size = if max_grant_size > 0 {
                rng.gen_range(1..=max_grant_size.min(remaining_data.len()))
            } else {
                continue; // No space available, try again later
            };

            let mut grant = publisher.grant(grant_size).unwrap();
            let bytes_to_copy = grant_size.min(remaining_data.len());
            grant[..bytes_to_copy].copy_from_slice(&remaining_data[..bytes_to_copy]);
            grant.commit(bytes_to_copy);

            total_sent += bytes_to_copy;

            // Notify subscribers
            for _ in 0..num_subscribers {
                let _ = producer.send(true);
            }
        }

        // Signal subscribers to stop
        for _ in 0..num_subscribers {
            let _ = producer.send(false);
        }
    };

    // Join subscriber threads and collect received data
    let mut all_received_data = Vec::new();
    let (_, received_data) = join(producer_fut, join_all(subscriber_handles)).await;
    for data in received_data {
        all_received_data.extend_from_slice(&data);
    }

    // Assertions
    assert_eq!(total_sent, data.len()); // Ensure all data was published
    assert_eq!(all_received_data, data); // Ensure all data was received
}
