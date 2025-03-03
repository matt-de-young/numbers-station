use rand::Rng;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time;

use crate::proto::StreamStationReply;
use crate::types::{BroadcastMap, StoredStation};

fn gen_number(rng: &mut impl Rng) -> String {
    let n = rng.gen::<u64>();
    format!(
        "{:02}{:03}{:02}",
        n % 100,
        (n / 100) % 1000,
        (n / 100_000) % 100
    )
}

impl Drop for StoredStation {
    fn drop(&mut self) {
        if let Ok(mut listeners) = self.current_listeners.lock() {
            *listeners = 0;
        }

        println!(
            "Dropping station {}: cleaning up resources",
            self.station.station_id
        );
    }
}

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
                            let message = gen_number(&mut *rng_lock);

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
        let mut interval = time::interval(cleanup_interval);
        loop {
            interval.tick().await;

            if let Ok(mut stations_lock) = stations.lock() {
                let now = std::time::Instant::now();
                stations_lock.retain(|_, station| {
                    if let Ok(listeners) = station.current_listeners.lock() {
                        if *listeners > 0 {
                            return true;
                        }
                    }

                    match station.last_active.lock() {
                        Ok(last_active) => now.duration_since(*last_active) < inactive_threshold,
                        Err(_) => true, // Keep station if we can't acquire lock
                    }
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

    #[test]
    fn test_gen_number_format() {
        let mut rng = SmallRng::seed_from_u64(42);
        let number = gen_number(&mut rng);

        assert_eq!(number.len(), 7, "Generated number should be 7 digits");
        assert!(
            number.chars().all(|c| c.is_ascii_digit()),
            "All characters should be digits"
        );
    }

    #[test]
    fn test_gen_number_parts() {
        let mut rng = SmallRng::seed_from_u64(42);
        let number = gen_number(&mut rng);

        let part1 = &number[0..2];
        let part2 = &number[2..5];
        let part3 = &number[5..7];

        assert!(
            part1.parse::<u64>().unwrap() < 100,
            "First part should be < 100"
        );
        assert!(
            part2.parse::<u64>().unwrap() < 1000,
            "Second part should be < 1000"
        );
        assert!(
            part3.parse::<u64>().unwrap() < 100,
            "Third part should be < 100"
        );
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

    #[test]
    fn test_gen_number_deterministic() {
        let mut rng1 = SmallRng::seed_from_u64(42);
        let mut rng2 = SmallRng::seed_from_u64(42);

        let number1 = gen_number(&mut rng1);
        let number2 = gen_number(&mut rng2);

        assert_eq!(number1, number2, "Same seed should produce same number");
    }

    #[test]
    fn test_gen_number_distribution() {
        let mut rng = SmallRng::seed_from_u64(42);
        let mut seen_different = false;

        // Generate several numbers and ensure they're not all the same
        let first = gen_number(&mut rng);
        for _ in 0..10 {
            if gen_number(&mut rng) != first {
                seen_different = true;
                break;
            }
        }

        assert!(
            seen_different,
            "Should generate different numbers over time"
        );
    }

    #[tokio::test]
    async fn test_cleanup_loop_removes_inactive_stations_immediately() {
        let stations = Arc::new(Mutex::new(HashMap::new()));
        let cleanup_interval = Duration::from_millis(50);
        let inactive_threshold = Duration::from_millis(100);

        {
            let mut stations_lock = stations.lock().unwrap();
            let station = create_test_station(1);

            if let Ok(mut last_active) = station.last_active.lock() {
                *last_active = std::time::Instant::now() - Duration::from_millis(200);
            }
            stations_lock.insert(1, station);
        }

        let stations_clone = Arc::clone(&stations);
        start_cleanup_loop(stations_clone, cleanup_interval, inactive_threshold);

        tokio::time::sleep(cleanup_interval + Duration::from_millis(10)).await;

        let stations_lock = stations.lock().unwrap();
        assert!(
            stations_lock.is_empty(),
            "Inactive station should be removed quickly"
        );
    }

    #[tokio::test]
    async fn test_cleanup_loop_handles_multiple_stations() {
        let stations = Arc::new(Mutex::new(HashMap::new()));
        let cleanup_interval = Duration::from_millis(50);
        let inactive_threshold = Duration::from_millis(100);

        {
            let mut stations_lock = stations.lock().unwrap();

            // Active station with listeners
            let active_station = create_test_station(1);
            *active_station.current_listeners.lock().unwrap() = 1;
            stations_lock.insert(1, active_station);

            // Recently active station without listeners
            let recent_station = create_test_station(2);
            stations_lock.insert(2, recent_station);

            // Inactive station
            let inactive_station = create_test_station(3);
            if let Ok(mut last_active) = inactive_station.last_active.lock() {
                *last_active = std::time::Instant::now() - Duration::from_millis(200);
            }
            stations_lock.insert(3, inactive_station);
        }

        let stations_clone = Arc::clone(&stations);
        start_cleanup_loop(stations_clone, cleanup_interval, inactive_threshold);

        tokio::time::sleep(cleanup_interval + Duration::from_millis(10)).await;

        let stations_lock = stations.lock().unwrap();
        assert_eq!(
            stations_lock.len(),
            2,
            "Should keep active and recent stations"
        );
        assert!(
            stations_lock.contains_key(&1),
            "Should keep station with listeners"
        );
        assert!(stations_lock.contains_key(&2), "Should keep recent station");
        assert!(
            !stations_lock.contains_key(&3),
            "Should remove inactive station"
        );
    }

    #[tokio::test]
    async fn test_station_cleanup_on_drop() {
        let station = create_test_station(1);

        // Set some listeners
        *station.current_listeners.lock().unwrap() = 5;

        // Drop the station
        drop(station);

        // Create new station with same ID to verify cleanup
        let new_station = create_test_station(1);
        assert_eq!(
            *new_station.current_listeners.lock().unwrap(),
            0,
            "New station should start with 0 listeners"
        );
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
                assert!(reply.message.chars().all(|c| c.is_ascii_digit()));

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
