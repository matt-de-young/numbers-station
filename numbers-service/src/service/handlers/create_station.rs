use prost_types::Timestamp;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

use crate::proto::numbers::{CreateStationReply, CreateStationRequest, Station};
use crate::types::StoredStation;

impl crate::service::NumbersService {
    pub async fn handle_create_station(
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
        drop(stations_lock);

        let station_id = self.generate_unique_station_id()?;

        let now = std::time::SystemTime::now();
        let since_epoch = now
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Status::internal("Time conversion error"))?;

        let timestamp = Timestamp {
            seconds: since_epoch.as_secs() as i64,
            nanos: since_epoch.subsec_nanos() as i32,
        };

        let seed = if let Some(seed_str) = req.seed {
            seed_str
                .parse::<i32>()
                .map_err(|_| Status::invalid_argument("Invalid seed value"))?
        } else {
            rand::random::<u8>() as i32
        };

        let mut seed_bytes = [0u8; 32];
        seed_bytes[0..8].copy_from_slice(&(seed as u64).to_le_bytes());
        let rng = SmallRng::from_seed(seed_bytes);

        let station = Station {
            station_id,
            created_at: Some(timestamp),
            current_listeners: 0,
        };

        let stored_station = StoredStation {
            station,
            rng: Arc::new(Mutex::new(rng)),
            current_listeners: Arc::new(Mutex::new(0)),
            last_active: Arc::new(Mutex::new(std::time::Instant::now())),
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
}
