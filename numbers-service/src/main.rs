use futures::Stream;
use prost_types::Timestamp;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

use proto::numbers_server::{Numbers, NumbersServer};
use proto::{
    CreateStationReply, CreateStationRequest, ListStationsReply, ListStationsRequest, Station,
    StreamStationReply, StreamStationRequest,
};

pub mod proto {
    tonic::include_proto!("numbers.v1");

    pub(crate) const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("numbers.v1");
}

type BroadcastSender = tokio::sync::mpsc::Sender<Result<StreamStationReply, Status>>;
type BroadcastMap = Arc<Mutex<HashMap<i32, Vec<BroadcastSender>>>>;

#[derive(Debug, Clone)]
struct StoredStation {
    station: Station,
    rng: Arc<Mutex<SmallRng>>,
    current_listeners: Arc<Mutex<u32>>,
    last_active: Arc<Mutex<std::time::Instant>>,
}

pub struct NumbersService {
    stations: Arc<Mutex<HashMap<i32, StoredStation>>>,
    number_broadcasts: BroadcastMap,
    max_stations: usize,
}

struct NotifyOnDrop {
    notify: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.notify.take() {
            let _ = tx.send(());
        }
    }
}

struct TrackedStream<S> {
    stream: S,
    _notify: NotifyOnDrop,
}

impl<S: Stream + Unpin> Stream for TrackedStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.stream).poll_next(cx)
    }
}

impl NumbersService {
    pub fn new(
        max_stations: usize,
        cleanup_interval: Duration,
        inactive_threshold: Duration,
    ) -> Self {
        let service = Self {
            stations: Arc::new(Mutex::new(HashMap::new())),
            number_broadcasts: Arc::new(Mutex::new(HashMap::new())),
            max_stations,
        };

        // Start the broadcast loop
        let stations = Arc::clone(&service.stations);
        let broadcasts = Arc::clone(&service.number_broadcasts);
        tokio::spawn(async move {
            loop {
                // Sleep until the next second
                let now = tokio::time::Instant::now();
                let next_second = std::time::Duration::from_nanos(
                    1_000_000_000 - (now.elapsed().as_nanos() as u64 % 1_000_000_000),
                );
                tokio::time::sleep(next_second).await;

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

        // Add cleanup loop
        let stations_cleanup = Arc::clone(&service.stations);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(cleanup_interval).await;

                if let Ok(mut stations_lock) = stations_cleanup.lock() {
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

        service
    }

    fn generate_unique_station_id(&self) -> Result<i32, Status> {
        let stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        for _ in 0..10 {
            // Generate random u16 and cast to i32 - will always be positive and ≤ 65535
            let id = rand::random::<u16>() as i32;
            if !stations_lock.contains_key(&id) {
                return Ok(id);
            }
        }

        Err(Status::internal("Failed to generate unique station ID"))
    }
}

#[tonic::async_trait]
impl Numbers for NumbersService {
    type StreamStationStream =
        Pin<Box<dyn Stream<Item = Result<StreamStationReply, Status>> + Send + 'static>>;

    async fn list_stations(
        &self,
        _request: Request<ListStationsRequest>,
    ) -> Result<Response<ListStationsReply>, Status> {
        let stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        let stations: Vec<Station> = stations_lock
            .values()
            .map(|internal| {
                let mut station = internal.station;
                // Update the current_listeners field
                if let Ok(listeners) = internal.current_listeners.lock() {
                    station.current_listeners = *listeners;
                }
                station
            })
            .collect();

        Ok(Response::new(ListStationsReply { stations }))
    }

    async fn create_station(
        &self,
        request: Request<CreateStationRequest>,
    ) -> Result<Response<CreateStationReply>, Status> {
        let req = request.into_inner();

        let stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;
        if stations_lock.len() >= self.max_stations {
            return Err(Status::resource_exhausted(
                "Maximum number of stations reached",
            ));
        }
        drop(stations_lock); // Release the lock before continuing

        let seed = if let Some(seed_str) = req.seed {
            seed_str
                .parse::<i32>()
                .map_err(|_| Status::invalid_argument("Invalid seed value"))?
        } else {
            rand::random::<u8>() as i32
        };

        let station_id = self.generate_unique_station_id()?;

        let now = std::time::SystemTime::now();
        let since_epoch = now
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Status::internal("Time conversion error"))?;

        let timestamp = Timestamp {
            seconds: since_epoch.as_secs() as i64,
            nanos: since_epoch.subsec_nanos() as i32,
        };

        // Initialize RNG with seed
        let mut seed_bytes = [0u8; 32];
        seed_bytes[0..8].copy_from_slice(&(seed as u64).to_le_bytes());
        let rng = SmallRng::from_seed(seed_bytes);

        let station = Station {
            station_id,
            created_at: Some(timestamp),
            current_listeners: 0, // Initialize with 0 listeners
        };

        let stored_station = StoredStation {
            station,
            rng: Arc::new(Mutex::new(rng)),
            current_listeners: Arc::new(Mutex::new(0)),
            last_active: Arc::new(Mutex::new(std::time::Instant::now())), // Initialize last_active
        };

        let mut stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;
        stations_lock.insert(station_id, stored_station);

        Ok(Response::new(CreateStationReply {
            station: Some(station),
        }))
    }

    async fn stream_station(
        &self,
        request: Request<StreamStationRequest>,
    ) -> Result<Response<Self::StreamStationStream>, Status> {
        let station_id = request.into_inner().station_id;

        let stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        let station = stations_lock
            .get(&station_id)
            .ok_or_else(|| Status::not_found("Station not found"))?;

        // Update last_active timestamp
        if let Ok(mut last_active) = station.last_active.lock() {
            *last_active = std::time::Instant::now();
        }

        // Increment listener count
        if let Ok(mut listeners) = station.current_listeners.lock() {
            *listeners += 1;
        }
        let current_listeners = Arc::clone(&station.current_listeners);
        drop(stations_lock);

        // Create channel for streaming
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        // Create oneshot channel for cleanup notification
        let (notify_tx, notify_rx) = tokio::sync::oneshot::channel();

        // Add to broadcast list
        let mut broadcasts_lock = self
            .number_broadcasts
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;
        broadcasts_lock
            .entry(station_id)
            .or_insert_with(Vec::new)
            .push(tx);

        // Create a task that will decrement the counter when the stream ends
        let broadcasts = Arc::clone(&self.number_broadcasts);
        tokio::spawn(async move {
            // Wait for the notification that the stream has ended
            let _ = notify_rx.await;

            // Decrement listener count
            if let Ok(mut listeners) = current_listeners.lock() {
                *listeners = listeners.saturating_sub(1);
            }

            // Clean up the broadcast sender
            if let Ok(mut broadcasts) = broadcasts.lock() {
                if let Some(txs) = broadcasts.get_mut(&station_id) {
                    txs.retain(|tx| !tx.is_closed());
                }
            }
        });

        // Create stream with drop notification
        let stream = ReceiverStream::new(rx);
        let tracked_stream = TrackedStream {
            stream,
            _notify: NotifyOnDrop {
                notify: Some(notify_tx),
            },
        };

        Ok(Response::new(Box::pin(tracked_stream)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let max_stations = std::env::var("MAX_STATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let cleanup_interval = tokio::time::Duration::from_secs(
        std::env::var("CLEANUP_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60),
    ); // 1 minute

    let inactive_threshold = std::time::Duration::from_secs(
        std::env::var("INACTIVE_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600),
    ); // 1 hour

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .unwrap();

    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<NumbersServer<NumbersService>>()
        .await;

    let addr = "[::1]:50051".parse()?;
    let numbers_service = NumbersService::new(max_stations, cleanup_interval, inactive_threshold);

    Server::builder()
        .add_service(reflection_service)
        .add_service(health_service)
        .add_service(NumbersServer::new(numbers_service))
        .serve(addr)
        .await?;

    Ok(())
}
