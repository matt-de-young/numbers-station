use crate::proto::numbers::{ListStationsReply, ListStationsRequest, Station};
use tonic::{Request, Response, Status};

impl crate::service::NumbersService {
    pub async fn handle_list_stations(
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
                if let Ok(listeners) = internal.current_listeners.lock() {
                    station.current_listeners = *listeners;
                }
                station
            })
            .collect();

        Ok(Response::new(ListStationsReply { stations }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::NumbersService;
    use crate::types::StoredStation;
    use prost_types::Timestamp;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn create_test_station(id: i32, listeners: u32) -> StoredStation {
        let timestamp = Timestamp {
            seconds: 0,
            nanos: 0,
        };

        let station = Station {
            station_id: id,
            created_at: Some(timestamp),
            current_listeners: listeners,
        };

        let seed_bytes = [0u8; 32];
        let rng = SmallRng::from_seed(seed_bytes);

        StoredStation {
            station,
            rng: Arc::new(Mutex::new(rng)),
            current_listeners: Arc::new(Mutex::new(listeners)),
            last_active: Arc::new(Mutex::new(Instant::now())),
        }
    }

    fn setup_service_with_stations(stations: Vec<(i32, u32)>) -> NumbersService {
        let mut stations_map = HashMap::new();
        for (id, listeners) in stations {
            stations_map.insert(id, create_test_station(id, listeners));
        }

        NumbersService {
            stations: Arc::new(Mutex::new(stations_map)),
            max_stations: 10,
            number_broadcasts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn test_list_empty_stations() {
        let service = setup_service_with_stations(vec![]);
        let request = Request::new(ListStationsRequest {});

        let response = service.handle_list_stations(request).await.unwrap();
        let reply = response.into_inner();

        assert!(reply.stations.is_empty());
    }

    #[tokio::test]
    async fn test_list_single_station() {
        let service = setup_service_with_stations(vec![(1, 5)]);
        let request = Request::new(ListStationsRequest {});

        let response = service.handle_list_stations(request).await.unwrap();
        let reply = response.into_inner();

        assert_eq!(reply.stations.len(), 1);
        assert_eq!(reply.stations[0].station_id, 1);
        assert_eq!(reply.stations[0].current_listeners, 5);
    }

    #[tokio::test]
    async fn test_list_multiple_stations() {
        let service = setup_service_with_stations(vec![(1, 5), (2, 10), (3, 15)]);
        let request = Request::new(ListStationsRequest {});

        let response = service.handle_list_stations(request).await.unwrap();
        let reply = response.into_inner();

        assert_eq!(reply.stations.len(), 3);

        let mut stations = reply.stations;
        stations.sort_by_key(|s| s.station_id);

        assert_eq!(stations[0].station_id, 1);
        assert_eq!(stations[0].current_listeners, 5);

        assert_eq!(stations[1].station_id, 2);
        assert_eq!(stations[1].current_listeners, 10);

        assert_eq!(stations[2].station_id, 3);
        assert_eq!(stations[2].current_listeners, 15);
    }

    #[tokio::test]
    async fn test_listener_count_updates() {
        let service = setup_service_with_stations(vec![(1, 5)]);

        let stations_lock = service.stations.lock().unwrap();
        let station = stations_lock.get(&1).unwrap();
        *station.current_listeners.lock().unwrap() = 7;
        drop(stations_lock);

        let request = Request::new(ListStationsRequest {});
        let response = service.handle_list_stations(request).await.unwrap();
        let reply = response.into_inner();

        assert_eq!(reply.stations.len(), 1);
        assert_eq!(reply.stations[0].station_id, 1);
        assert_eq!(reply.stations[0].current_listeners, 7);
    }

    #[tokio::test]
    async fn test_timestamp_presence() {
        let service = setup_service_with_stations(vec![(1, 5)]);
        let request = Request::new(ListStationsRequest {});

        let response = service.handle_list_stations(request).await.unwrap();
        let reply = response.into_inner();

        assert!(reply.stations[0].created_at.is_some());
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let service = Arc::new(setup_service_with_stations(vec![(1, 5)]));
        let service_clone = service.clone();

        let update_task = tokio::spawn(async move {
            let stations_lock = service_clone.stations.lock().unwrap();
            let station = stations_lock.get(&1).unwrap();
            *station.current_listeners.lock().unwrap() = 10;
        });

        let request = Request::new(ListStationsRequest {});
        let response = service.handle_list_stations(request).await.unwrap();

        update_task.await.unwrap();

        let reply = response.into_inner();
        assert_eq!(reply.stations.len(), 1);
        // Note: We can't assert the exact listener count here as it depends on timing
        assert!(
            reply.stations[0].current_listeners == 5 || reply.stations[0].current_listeners == 10
        );
    }
}
