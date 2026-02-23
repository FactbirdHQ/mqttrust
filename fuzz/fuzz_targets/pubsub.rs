#![no_main]
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Duration;
use mqttrust::{pubsub::PubSubChannel, SliceBufferProvider};

use futures::future::{join, join_all};
use libfuzzer_sys::fuzz_target;
use rand::Rng;
use tokio::{sync::broadcast, task::yield_now};

const MAX_SUBSCRIBERS: usize = 64;

fuzz_target!(|data: &[u8]| fuzz(data));

fn fuzz(data: &[u8]) {
    // if std::env::var_os("RUST_LOG").is_some() {
    //     env_logger::init();
    // }
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(fuzz_inner(data))
}

async fn fuzz_inner(data: &[u8]) {
    let mut slice = [0u8; 1024];
    let pubsub: PubSubChannel<CriticalSectionRawMutex, SliceBufferProvider, MAX_SUBSCRIBERS> =
        PubSubChannel::new(SliceBufferProvider::new(&mut slice));

    let (producer, _) = broadcast::channel(1);

    let num_subscribers = if !data.is_empty() {
        (data[0] % MAX_SUBSCRIBERS as u8) as usize + 1 // at least 1 subscriber
    } else {
        1
    };
    let barrier = tokio::sync::Barrier::new(num_subscribers + 1); // 3 subscribers + 1 publisher

    // Spawn subscribers
    let mut subscriber_handles = Vec::new();
    for _ in 0..num_subscribers {
        let barrier_ref = &barrier;

        subscriber_handles.push(async {
            let mut subscriber = pubsub.subscriber().unwrap();
            let mut terminate = producer.subscribe();
            barrier_ref.wait().await;
            let mut received_data = Vec::new();
            loop {
                match select(subscriber.read_async(), terminate.recv()).await {
                    Either::First(Ok(grant)) => {
                        received_data.extend_from_slice(grant.buf());
                        grant.release();
                    }
                    Either::First(Err(e)) => println!("Error: {:?}", e),
                    Either::Second(_) => break,
                }
            }
            received_data
        });
    }

    // Producer thread
    let mut total_sent = 0;
    let producer_fut = embassy_time::with_timeout(Duration::from_secs(3), async {
        let mut publisher = pubsub.publisher().unwrap();
        let mut rng = rand::thread_rng();

        barrier.wait().await;

        println!(
            "Starting fuzz. Data len: {}, subscribers: {}",
            data.len(),
            num_subscribers
        );
        while total_sent < data.len() {
            let remaining_data = &data[total_sent..];

            // Generate random data size to publish
            let max_grant_size = publisher.free_space();
            let grant_size = if max_grant_size > 0 && !publisher.is_full() {
                rng.gen_range(1..=max_grant_size.min(remaining_data.len()))
            } else {
                yield_now().await;
                continue; // No space available, try again later
            };

            let mut grant = publisher.grant(grant_size).unwrap();
            let bytes_to_copy = grant_size.min(remaining_data.len());
            grant[..bytes_to_copy].copy_from_slice(&remaining_data[..bytes_to_copy]);
            grant.commit(bytes_to_copy);

            total_sent += bytes_to_copy;
        }

        // Signal subscribers to stop
        let _ = producer.send(());
    });

    // Join subscriber threads and collect received data
    let (_, received_data) = join(producer_fut, join_all(subscriber_handles)).await;

    // Assertions
    assert_eq!(total_sent, data.len()); // Ensure all data was published
    for sub_received_data in received_data {
        assert_eq!(sub_received_data, data); // Ensure all data was received
    }
}
