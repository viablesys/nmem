use opentelemetry::KeyValue;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::Resource;
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize, Default, Clone)]
pub struct MetricsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
}

fn default_transport() -> String {
    "http".into()
}

fn default_endpoint() -> String {
    "http://localhost:8428/opentelemetry/v1/metrics".into()
}

/// Build and register a global meter provider.
/// For gRPC transport, requires an active tokio runtime context.
pub fn init_meter_provider(config: &MetricsConfig) -> Option<SdkMeterProvider> {
    if !config.enabled {
        return None;
    }

    let resource = Resource::builder_empty()
        .with_attributes([KeyValue::new("service.name", "nmem")])
        .build();

    let provider = match config.transport.as_str() {
        "grpc" => {
            let exporter = opentelemetry_otlp::MetricExporter::builder()
                .with_tonic()
                .with_endpoint(&config.endpoint)
                .with_protocol(Protocol::Grpc)
                .with_timeout(Duration::from_secs(5))
                .build()
                .map_err(|e| eprintln!("nmem: metrics grpc exporter: {e}"))
                .ok()?;
            SdkMeterProvider::builder()
                .with_periodic_exporter(exporter)
                .with_resource(resource)
                .build()
        }
        _ => {
            let exporter = opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                .with_protocol(Protocol::HttpBinary)
                .with_endpoint(&config.endpoint)
                .with_timeout(Duration::from_secs(5))
                .build()
                .map_err(|e| eprintln!("nmem: metrics http exporter: {e}"))
                .ok()?;
            SdkMeterProvider::builder()
                .with_periodic_exporter(exporter)
                .with_resource(resource)
                .build()
        }
    };

    opentelemetry::global::set_meter_provider(provider.clone());
    Some(provider)
}
