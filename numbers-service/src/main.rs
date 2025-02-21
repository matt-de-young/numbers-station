use prost_types::Timestamp;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

use numbers::numbers_server::{Numbers, NumbersServer};
use numbers::{
    CreateStationReply, CreateStationRequest, ListStationsReply, ListStationsRequest, Station,
    StreamStationReply, StreamStationRequest,
};

pub mod numbers {
    tonic::include_proto!("numbers.v1");
}

#[derive(Debug, Clone)]
struct StoredStation {
    station: Station,
    rng: Arc<Mutex<SmallRng>>, // Add RNG here
}

pub struct NumbersService {
    stations: Arc<Mutex<HashMap<i32, StoredStation>>>,
    number_broadcasts: Arc<
        Mutex<HashMap<i32, Vec<tokio::sync::mpsc::Sender<Result<StreamStationReply, Status>>>>>,
    >,
}

impl Default for NumbersService {
    fn default() -> Self {
        let service = Self {
            stations: Arc::new(Mutex::new(HashMap::new())),
            number_broadcasts: Arc::new(Mutex::new(HashMap::new())),
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

        service
    }
}

impl NumbersService {
    // Helper function to generate a unique random station ID
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
            .map(|internal| internal.station)
            .collect();

        Ok(Response::new(ListStationsReply { stations }))
    }

    async fn create_station(
        &self,
        request: Request<CreateStationRequest>,
    ) -> Result<Response<CreateStationReply>, Status> {
        let req = request.into_inner();

        let seed = if let Some(seed_str) = req.seed {
            seed_str
                .parse::<i32>()
                .map_err(|_| Status::invalid_argument("Invalid seed value"))?
        } else {
            rand::random::<u8>() as i32
        };

        // Generate random unique station ID
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
        };

        let internal_station = StoredStation {
            station,
            rng: Arc::new(Mutex::new(rng)),
        };

        let mut stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;
        stations_lock.insert(station_id, internal_station);

        Ok(Response::new(CreateStationReply {
            station: Some(station),
        }))
    }

    async fn stream_station(
        &self,
        request: Request<StreamStationRequest>,
    ) -> Result<Response<Self::StreamStationStream>, Status> {
        let station_id = request.into_inner().station_id;

        // Verify station exists
        let stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;
        if !stations_lock.contains_key(&station_id) {
            return Err(Status::not_found("Station not found"));
        }
        drop(stations_lock);

        // Create channel for streaming
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        // Add to broadcast list
        let mut broadcasts_lock = self
            .number_broadcasts
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;
        broadcasts_lock
            .entry(station_id)
            .or_insert_with(Vec::new)
            .push(tx);

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    type StreamStationStream = ReceiverStream<Result<StreamStationReply, Status>>;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse()?;
    let numbers_service = NumbersService::default();

    Server::builder()
        .add_service(NumbersServer::new(numbers_service))
        .serve(addr)
        .await?;

    Ok(())
}
