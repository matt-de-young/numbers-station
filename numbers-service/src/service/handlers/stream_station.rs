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
        let (tx, rx) = mpsc::channel(32);

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
