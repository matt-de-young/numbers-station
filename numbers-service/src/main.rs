mod proto;
mod service;
mod types;

use crate::proto::numbers::numbers_server::NumbersServer;
use figment::{providers::Env, Figment};
use serde::Deserialize;
use service::NumbersService;
use std::time::Duration;
use tonic::transport::Server;

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default = "default_max_stations")]
    max_stations: usize,

    #[serde(default = "default_cleanup_interval")]
    cleanup_interval_secs: u64,

    #[serde(default = "default_inactive_threshold")]
    inactive_threshold_secs: u64,

    #[serde(default = "default_address")]
    address: String,
}

fn default_max_stations() -> usize {
    100
}
fn default_cleanup_interval() -> u64 {
    60
}
fn default_inactive_threshold() -> u64 {
    3600
}
fn default_address() -> String {
    "[::1]:50051".to_string()
}

impl Config {
    fn cleanup_interval(&self) -> Duration {
        Duration::from_secs(self.cleanup_interval_secs)
    }

    fn inactive_threshold(&self) -> Duration {
        Duration::from_secs(self.inactive_threshold_secs)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = Figment::new()
        .join(Env::prefixed("NUMBERS_").split("_"))
        .extract()?;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .unwrap();

    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<NumbersServer<NumbersService>>()
        .await;

    let addr = config.address.parse()?;
    let numbers_service = NumbersService::new(
        config.max_stations,
        config.cleanup_interval(),
        config.inactive_threshold(),
    );

    Server::builder()
        .add_service(reflection_service)
        .add_service(health_service)
        .add_service(NumbersServer::new(numbers_service))
        .serve(addr)
        .await?;

    Ok(())
}
