mod proto;
mod service;
mod types;

use crate::proto::numbers::numbers_server::NumbersServer;
use service::NumbersService;
use std::time::Duration;
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let max_stations = std::env::var("MAX_STATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let cleanup_interval = Duration::from_secs(
        std::env::var("CLEANUP_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60),
    );

    let inactive_threshold = Duration::from_secs(
        std::env::var("INACTIVE_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600),
    );

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
