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
