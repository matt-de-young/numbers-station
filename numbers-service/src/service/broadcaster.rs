use rand::Rng;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time;

use crate::proto::StreamStationReply;
use crate::types::{BroadcastMap, StoredStation};

pub fn start_broadcast_loop(
    stations: Arc<Mutex<HashMap<i32, StoredStation>>>,
    broadcasts: BroadcastMap,
) {
    tokio::spawn(async move {
        loop {
            let now = time::Instant::now();
            let next_second = Duration::from_nanos(
                1_000_000_000 - (now.elapsed().as_nanos() as u64 % 1_000_000_000),
            );
            time::sleep(next_second).await;

            if let Ok(stations_lock) = stations.lock() {
                if let Ok(mut broadcasts_lock) = broadcasts.lock() {
                    for txs in broadcasts_lock.values_mut() {
                        txs.retain(|tx| !tx.is_closed());
                    }

                    for (station_id, station) in stations_lock.iter() {
                        if let Ok(mut rng_lock) = station.rng.lock() {
                            let n1 = rng_lock.gen::<u64>();
                            let n2 = rng_lock.gen::<u64>();
                            let n3 = rng_lock.gen::<u64>();

                            let part1 = n1 % 100;
                            let part2 = n2 % 1000;
                            let part3 = n3 % 100;

                            let message = format!("{:02}{:03}{:02}", part1, part2, part3);

                            if let Some(txs) = broadcasts_lock.get(station_id) {
                                let reply = StreamStationReply { message };
                                for tx in txs {
                                    let _ = tx.try_send(Ok(reply.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
    });
}

pub fn start_cleanup_loop(
    stations: Arc<Mutex<HashMap<i32, StoredStation>>>,
    cleanup_interval: Duration,
    inactive_threshold: Duration,
) {
    tokio::spawn(async move {
        loop {
            time::sleep(cleanup_interval).await;

            if let Ok(mut stations_lock) = stations.lock() {
                let now = std::time::Instant::now();
                stations_lock.retain(|_, station| {
                    let keep = if let (Ok(listeners), Ok(last_active)) =
                        (station.current_listeners.lock(), station.last_active.lock())
                    {
                        *listeners > 0 || now.duration_since(*last_active) < inactive_threshold
                    } else {
                        true
                    };
                    keep
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::numbers::Station;
    use prost_types::Timestamp;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;
    use tokio::sync::mpsc;

    fn create_test_station(id: i32) -> StoredStation {
        let timestamp = Timestamp {
            seconds: 0,
            nanos: 0,
        };

        let station = Station {
            station_id: id,
            created_at: Some(timestamp),
            current_listeners: 0,
        };

        let seed_bytes = [0u8; 32];
        let rng = SmallRng::from_seed(seed_bytes);

        StoredStation {
            station,
            rng: Arc::new(Mutex::new(rng)),
            current_listeners: Arc::new(Mutex::new(0)),
            last_active: Arc::new(Mutex::new(std::time::Instant::now())),
        }
    }

    #[tokio::test]
    async fn test_cleanup_loop_removes_inactive_stations() {
        let stations = Arc::new(Mutex::new(HashMap::new()));
        let cleanup_interval = Duration::from_millis(100);
        let inactive_threshold = Duration::from_millis(200);

        {
            let mut stations_lock = stations.lock().unwrap();
            stations_lock.insert(1, create_test_station(1));
        }

        let stations_clone = Arc::clone(&stations);
        start_cleanup_loop(stations_clone, cleanup_interval, inactive_threshold);

        tokio::time::sleep(cleanup_interval + inactive_threshold).await;

        let stations_lock = stations.lock().unwrap();
        assert!(
            stations_lock.is_empty(),
            "Inactive station should be removed"
        );
    }

    #[tokio::test]
    async fn test_cleanup_loop_keeps_active_stations() {
        let stations = Arc::new(Mutex::new(HashMap::new()));
        let cleanup_interval = Duration::from_millis(100);
        let inactive_threshold = Duration::from_millis(200);

        {
            let mut stations_lock = stations.lock().unwrap();
            let station = create_test_station(1);
            *station.current_listeners.lock().unwrap() = 1;
            stations_lock.insert(1, station);
        }

        let stations_clone = Arc::clone(&stations);
        start_cleanup_loop(stations_clone, cleanup_interval, inactive_threshold);

        tokio::time::sleep(cleanup_interval + inactive_threshold).await;

        let stations_lock = stations.lock().unwrap();
        assert!(!stations_lock.is_empty(), "Active station should be kept");
    }

    #[tokio::test]
    async fn test_broadcast_loop_sends_messages() {
        let stations = Arc::new(Mutex::new(HashMap::new()));
        let broadcasts: BroadcastMap = Arc::new(Mutex::new(HashMap::new()));

        let (tx, mut rx) = mpsc::channel(100);

        {
            let mut stations_lock = stations.lock().unwrap();
            stations_lock.insert(1, create_test_station(1));

            let mut broadcasts_lock = broadcasts.lock().unwrap();
            broadcasts_lock.insert(1, vec![tx]);
        }

        let stations_clone = Arc::clone(&stations);
        let broadcasts_clone = Arc::clone(&broadcasts);
        start_broadcast_loop(stations_clone, broadcasts_clone);

        // Wait for up to 2 seconds for a message
        let mut received_message = false;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match rx.try_recv() {
                Ok(Ok(reply)) => {
                    assert_eq!(reply.message.len(), 7, "Message should be 7 digits");
                    assert!(
                        reply.message.chars().all(|c| c.is_digit(10)),
                        "Message should be numeric"
                    );
                    received_message = true;
                    break;
                }
                _ => continue,
            }
        }
        assert!(received_message, "Should have received a broadcast message");
    }

    #[tokio::test]
    async fn test_broadcast_message_format() {
        let stations = Arc::new(Mutex::new(HashMap::new()));
        let broadcasts: BroadcastMap = Arc::new(Mutex::new(HashMap::new()));

        let (tx, mut rx) = mpsc::channel(100);

        {
            let mut stations_lock = stations.lock().unwrap();
            let station = create_test_station(1);
            *station.rng.lock().unwrap() = SmallRng::seed_from_u64(42);
            stations_lock.insert(1, station);

            let mut broadcasts_lock = broadcasts.lock().unwrap();
            broadcasts_lock.insert(1, vec![tx]);
        }

        let stations_clone = Arc::clone(&stations);
        let broadcasts_clone = Arc::clone(&broadcasts);
        start_broadcast_loop(stations_clone, broadcasts_clone);

        // Wait for up to 2 seconds for a message
        let mut received_message = false;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(Ok(reply)) = rx.try_recv() {
                assert_eq!(reply.message.len(), 7);
                assert!(reply.message.chars().all(|c| c.is_digit(10)));

                let part1 = &reply.message[0..2];
                let part2 = &reply.message[2..5];
                let part3 = &reply.message[5..7];

                assert!(part1.parse::<u64>().unwrap() < 100);
                assert!(part2.parse::<u64>().unwrap() < 1000);
                assert!(part3.parse::<u64>().unwrap() < 100);

                received_message = true;
                break;
            }
        }
        assert!(received_message, "Should have received a broadcast message");
    }
}
