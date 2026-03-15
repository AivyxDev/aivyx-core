use serde::{Deserialize, Serialize};

/// OpenTelemetry tracing configuration.
///
/// Controls distributed trace export to any OTel-compatible backend
/// (Langfuse, Datadog, Grafana Tempo, Jaeger). Stored as `[otel]` in TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelConfig {
    /// Export protocol: "otlp-grpc", "otlp-http", "stdout", "none".
    #[serde(default = "default_exporter")]
    pub exporter: String,
    /// OTLP endpoint (e.g., "http://localhost:4317" for gRPC,
    /// "http://localhost:4318" for HTTP).
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    /// Service name for the OTel resource attribute.
    #[serde(default = "default_service_name")]
    pub service_name: String,
    /// Sampling ratio: 1.0 = sample everything, 0.1 = 10%.
    #[serde(default = "default_sample_ratio")]
    pub sample_ratio: f64,
    /// Additional resource attributes as `key=value` pairs.
    #[serde(default)]
    pub resource_attributes: Vec<String>,
    /// Optional authorization header value for OTLP exporter
    /// (e.g., for Langfuse, Datadog, or Grafana Cloud).
    #[serde(default)]
    pub auth_header: Option<String>,
}

fn default_exporter() -> String {
    "otlp-grpc".into()
}

fn default_endpoint() -> String {
    "http://localhost:4317".into()
}

fn default_service_name() -> String {
    "aivyx-engine".into()
}

fn default_sample_ratio() -> f64 {
    1.0
}

impl Default for OtelConfig {
    fn default() -> Self {
        Self {
            exporter: default_exporter(),
            endpoint: default_endpoint(),
            service_name: default_service_name(),
            sample_ratio: default_sample_ratio(),
            resource_attributes: Vec::new(),
            auth_header: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = OtelConfig::default();
        assert_eq!(config.exporter, "otlp-grpc");
        assert_eq!(config.endpoint, "http://localhost:4317");
        assert_eq!(config.service_name, "aivyx-engine");
        assert!((config.sample_ratio - 1.0).abs() < f64::EPSILON);
        assert!(config.resource_attributes.is_empty());
        assert!(config.auth_header.is_none());
    }

    #[test]
    fn serde_roundtrip() {
        let config = OtelConfig {
            exporter: "otlp-http".into(),
            endpoint: "http://tempo:4318".into(),
            service_name: "my-app".into(),
            sample_ratio: 0.5,
            resource_attributes: vec!["env=prod".into(), "team=platform".into()],
            auth_header: Some("Bearer abc123".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: OtelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.exporter, "otlp-http");
        assert_eq!(parsed.endpoint, "http://tempo:4318");
        assert_eq!(parsed.service_name, "my-app");
        assert!((parsed.sample_ratio - 0.5).abs() < f64::EPSILON);
        assert_eq!(parsed.resource_attributes.len(), 2);
        assert_eq!(parsed.auth_header, Some("Bearer abc123".into()));
    }

    #[test]
    fn toml_roundtrip() {
        let config = OtelConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: OtelConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.exporter, "otlp-grpc");
        assert_eq!(parsed.endpoint, "http://localhost:4317");
        assert_eq!(parsed.service_name, "aivyx-engine");
    }

    #[test]
    fn empty_section_uses_defaults() {
        let parsed: OtelConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.exporter, "otlp-grpc");
        assert_eq!(parsed.endpoint, "http://localhost:4317");
        assert_eq!(parsed.service_name, "aivyx-engine");
        assert!((parsed.sample_ratio - 1.0).abs() < f64::EPSILON);
        assert!(parsed.resource_attributes.is_empty());
        assert!(parsed.auth_header.is_none());
    }
}
