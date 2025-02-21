mod broadcaster;
mod handlers;
mod stream;

use futures::Stream;
use rand::random;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::{Request, Response, Status};

use self::broadcaster::{start_broadcast_loop, start_cleanup_loop};
use crate::proto::numbers::numbers_server::Numbers;
use crate::proto::numbers::{
    CreateStationReply, CreateStationRequest, ListStationsReply, ListStationsRequest,
    StreamStationReply, StreamStationRequest,
};
use crate::types::{BroadcastMap, StoredStation};

pub struct NumbersService {
    stations: Arc<Mutex<HashMap<i32, StoredStation>>>,
    number_broadcasts: BroadcastMap,
    max_stations: usize,
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
        start_broadcast_loop(
            Arc::clone(&service.stations),
            Arc::clone(&service.number_broadcasts),
        );

        // Start cleanup loop
        start_cleanup_loop(
            Arc::clone(&service.stations),
            cleanup_interval,
            inactive_threshold,
        );

        service
    }

    fn generate_unique_station_id(&self) -> Result<i32, Status> {
        let stations_lock = self
            .stations
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        for _ in 0..10 {
            let id = random::<u16>() as i32;
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
        request: Request<ListStationsRequest>,
    ) -> Result<Response<ListStationsReply>, Status> {
        self.handle_list_stations(request).await
    }

    async fn create_station(
        &self,
        request: Request<CreateStationRequest>,
    ) -> Result<Response<CreateStationReply>, Status> {
        self.handle_create_station(request).await
    }

    async fn stream_station(
        &self,
        request: Request<StreamStationRequest>,
    ) -> Result<Response<Self::StreamStationStream>, Status> {
        self.handle_stream_station(request).await
    }
}
