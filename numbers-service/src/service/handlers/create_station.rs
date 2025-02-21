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

        let seed = if let Some(seed_value) = req.seed {
            if seed_value <= 0 {
                return Err(Status::invalid_argument("Invalid seed value"));
            }
            seed_value
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::NumbersService;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn setup_service(max_stations: usize) -> NumbersService {
        NumbersService {
            stations: Arc::new(Mutex::new(HashMap::new())),
            max_stations,
            number_broadcasts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn create_station(
        service: &NumbersService,
        seed: Option<i32>,
    ) -> Result<Station, Status> {
        let request = Request::new(CreateStationRequest { seed });
        let response = service.handle_create_station(request).await?;
        Ok(response.into_inner().station.unwrap())
    }

    async fn create_station_expect_error(service: &NumbersService, seed: Option<i32>) -> Status {
        let request = Request::new(CreateStationRequest { seed });
        service.handle_create_station(request).await.unwrap_err()
    }

    #[tokio::test]
    async fn test_create_station_success() {
        let service = setup_service(10);
        let station = create_station(&service, None).await.unwrap();

        assert!(station.station_id > 0);
        assert!(station.created_at.is_some());
        assert_eq!(station.current_listeners, 0);

        let stations_lock = service.stations.lock().unwrap();
        assert_eq!(stations_lock.len(), 1);
        assert!(stations_lock.contains_key(&station.station_id));
    }

    #[tokio::test]
    async fn test_create_station_with_seed() {
        let service = setup_service(10);
        let station1_id = create_station(&service, Some(42)).await.unwrap().station_id;
        let station2_id = create_station(&service, Some(42)).await.unwrap().station_id;

        let stations_lock = service.stations.lock().unwrap();
        let station1 = stations_lock.get(&station1_id).unwrap();
        let station2 = stations_lock.get(&station2_id).unwrap();

        let mut rng1 = station1.rng.lock().unwrap();
        let mut rng2 = station2.rng.lock().unwrap();

        use rand::Rng;
        for _ in 0..5 {
            let num1: u32 = rng1.gen();
            let num2: u32 = rng2.gen();
            assert_eq!(
                num1, num2,
                "RNGs with same seed should generate same numbers"
            );
        }
    }

    #[tokio::test]
    async fn test_create_station_max_limit() {
        let service = setup_service(1);

        let station1 = create_station(&service, None).await.unwrap();
        assert!(station1.station_id > 0);

        let error = create_station_expect_error(&service, None).await;
        assert_eq!(error.code(), tonic::Code::ResourceExhausted);
    }

    #[tokio::test]
    async fn test_negative_seed() {
        let service = setup_service(10);
        let error = create_station_expect_error(&service, Some(-42)).await;
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_different_seeds_different_sequences() {
        let service = setup_service(10);

        let station1_id = create_station(&service, Some(42)).await.unwrap().station_id;
        let station2_id = create_station(&service, Some(43)).await.unwrap().station_id;

        let stations_lock = service.stations.lock().unwrap();
        let station1 = stations_lock.get(&station1_id).unwrap();
        let station2 = stations_lock.get(&station2_id).unwrap();

        let mut rng1 = station1.rng.lock().unwrap();
        let mut rng2 = station2.rng.lock().unwrap();

        use rand::Rng;
        let mut all_numbers_same = true;
        for _ in 0..5 {
            let num1: u32 = rng1.gen();
            let num2: u32 = rng2.gen();
            if num1 != num2 {
                all_numbers_same = false;
                break;
            }
        }
        assert!(
            !all_numbers_same,
            "Different seeds should generate different sequences"
        );
    }
}
