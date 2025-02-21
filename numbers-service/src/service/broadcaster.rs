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
            // Sleep until the next second
            let now = time::Instant::now();
            let next_second = Duration::from_nanos(
                1_000_000_000 - (now.elapsed().as_nanos() as u64 % 1_000_000_000),
            );
            time::sleep(next_second).await;

            // Generate and broadcast numbers for all stations
            if let Ok(stations_lock) = stations.lock() {
                if let Ok(mut broadcasts_lock) = broadcasts.lock() {
                    // Clean up disconnected clients
                    for txs in broadcasts_lock.values_mut() {
                        txs.retain(|tx| !tx.is_closed());
                    }

                    // Generate and broadcast new numbers
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
                        true // Keep station if we can't acquire locks
                    };
                    keep
                });
            }
        }
    });
}
