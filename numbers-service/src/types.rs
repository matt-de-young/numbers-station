use crate::proto::numbers::{Station, StreamStationReply};
use futures::Stream;
use rand::rngs::SmallRng;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tonic::Status;

pub type BroadcastSender = tokio::sync::mpsc::Sender<Result<StreamStationReply, Status>>;
pub type BroadcastMap = Arc<Mutex<HashMap<i32, Vec<BroadcastSender>>>>;
pub type StreamResult =
    Pin<Box<dyn Stream<Item = Result<StreamStationReply, Status>> + Send + 'static>>;

#[derive(Debug, Clone)]
pub struct StoredStation {
    pub station: Station,
    pub rng: Arc<Mutex<SmallRng>>,
    pub current_listeners: Arc<Mutex<u32>>,
    pub last_active: Arc<Mutex<std::time::Instant>>,
}
