use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::proto::numbers::StreamStationRequest;
use crate::service::stream::{NotifyOnDrop, TrackedStream};
use crate::types::StreamResult;

impl crate::service::NumbersService {
    pub async fn handle_stream_station(
        &self,
        request: Request<StreamStationRequest>,
    ) -> Result<Response<StreamResult>, Status> {
        let station_id = request.into_inner().station_id;

        let stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        let station = stations_lock
            .get(&station_id)
            .ok_or_else(|| Status::not_found("Station not found"))?;

        if let Ok(mut last_active) = station.last_active.lock() {
            *last_active = std::time::Instant::now();
        }

        if let Ok(mut listeners) = station.current_listeners.lock() {
            *listeners += 1;
        }
        let current_listeners = Arc::clone(&station.current_listeners);
        drop(stations_lock);

        let (tx, rx) = mpsc::channel(32);
        let (notify_tx, notify_rx) = tokio::sync::oneshot::channel();

        let mut broadcasts_lock = self
            .number_broadcasts
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;
        broadcasts_lock
            .entry(station_id)
            .or_insert_with(Vec::new)
            .push(tx);

        let broadcasts = Arc::clone(&self.number_broadcasts);
        tokio::spawn(async move {
            let _ = notify_rx.await;

            if let Ok(mut listeners) = current_listeners.lock() {
                *listeners = listeners.saturating_sub(1);
            }

            if let Ok(mut broadcasts) = broadcasts.lock() {
                if let Some(txs) = broadcasts.get_mut(&station_id) {
                    txs.retain(|tx| !tx.is_closed());
                }
            }
        });

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::numbers::Station;
    use crate::service::NumbersService;
    use crate::types::StoredStation;
    use prost_types::Timestamp;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Instant;

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
            last_active: Arc::new(Mutex::new(Instant::now())),
        }
    }

    fn setup_service_with_station(station_id: i32) -> NumbersService {
        let mut stations = HashMap::new();
        stations.insert(station_id, create_test_station(station_id));

        NumbersService {
            stations: Arc::new(Mutex::new(stations)),
            max_stations: 10,
            number_broadcasts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn test_stream_nonexistent_station() {
        let service = setup_service_with_station(1);
        let request = Request::new(StreamStationRequest { station_id: 999 });

        match service.handle_stream_station(request).await {
            Err(status) => {
                assert_eq!(status.code(), tonic::Code::NotFound);
            }
            Ok(_) => panic!("Expected error for nonexistent station"),
        }
    }

    #[tokio::test]
    async fn test_stream_station_listener_count() {
        let service = setup_service_with_station(1);
        let request = Request::new(StreamStationRequest { station_id: 1 });

        let response = service.handle_stream_station(request).await.unwrap();

        {
            let stations_lock = service.stations.lock().unwrap();
            let station = stations_lock.get(&1).unwrap();
            assert_eq!(*station.current_listeners.lock().unwrap(), 1);
        }

        drop(response);

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let stations_lock = service.stations.lock().unwrap();
        let station = stations_lock.get(&1).unwrap();
        assert_eq!(*station.current_listeners.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_multiple_streams_same_station() {
        let service = setup_service_with_station(1);

        let request1 = Request::new(StreamStationRequest { station_id: 1 });
        let _response1 = service.handle_stream_station(request1).await.unwrap();

        let request2 = Request::new(StreamStationRequest { station_id: 1 });
        let _response2 = service.handle_stream_station(request2).await.unwrap();

        let stations_lock = service.stations.lock().unwrap();
        let station = stations_lock.get(&1).unwrap();
        assert_eq!(*station.current_listeners.lock().unwrap(), 2);

        let broadcasts_lock = service.number_broadcasts.lock().unwrap();
        assert_eq!(broadcasts_lock.get(&1).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_last_active_update() {
        let service = setup_service_with_station(1);

        let initial_time = {
            let stations_lock = service.stations.lock().unwrap();
            let station = stations_lock.get(&1).unwrap();
            let time = station.last_active.lock().unwrap();
            *time
        };

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let request = Request::new(StreamStationRequest { station_id: 1 });
        let _response = service.handle_stream_station(request).await.unwrap();

        let new_time = {
            let stations_lock = service.stations.lock().unwrap();
            let station = stations_lock.get(&1).unwrap();
            let time = station.last_active.lock().unwrap();
            *time
        };

        assert!(new_time > initial_time);
    }

    #[tokio::test]
    async fn test_broadcast_cleanup() {
        let service = setup_service_with_station(1);

        let request = Request::new(StreamStationRequest { station_id: 1 });
        let response = service.handle_stream_station(request).await.unwrap();
        drop(response);

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let broadcasts_lock = service.number_broadcasts.lock().unwrap();
        let broadcasts = broadcasts_lock.get(&1).unwrap();
        assert!(broadcasts.iter().all(|tx| tx.is_closed()));
    }

    #[tokio::test]
    async fn test_concurrent_streams() {
        let service = Arc::new(setup_service_with_station(1));
        let service_clone1 = Arc::clone(&service);
        let service_clone2 = Arc::clone(&service);

        let handle1 = tokio::spawn(async move {
            let request = Request::new(StreamStationRequest { station_id: 1 });
            service_clone1.handle_stream_station(request).await.unwrap()
        });

        let handle2 = tokio::spawn(async move {
            let request = Request::new(StreamStationRequest { station_id: 1 });
            service_clone2.handle_stream_station(request).await.unwrap()
        });

        let (_response1, _response2) = tokio::join!(handle1, handle2);

        let stations_lock = service.stations.lock().unwrap();
        let station = stations_lock.get(&1).unwrap();
        assert_eq!(*station.current_listeners.lock().unwrap(), 2);
    }
}
