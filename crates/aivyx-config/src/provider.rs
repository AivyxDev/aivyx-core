use serde::{Deserialize, Serialize};

/// LLM provider configuration.
///
/// API keys are stored by reference — `api_key_ref` is a name pointing into the
/// encrypted store, never the actual secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderConfig {
    Claude {
        /// Name of the API key in the encrypted store.
        api_key_ref: String,
        /// Model identifier (e.g., "claude-sonnet-4-20250514").
        model: String,
    },
    OpenAI {
        /// Name of the API key in the encrypted store.
        api_key_ref: String,
        /// Model identifier (e.g., "gpt-4o").
        model: String,
    },
    Ollama {
        /// Base URL of the Ollama server.
        base_url: String,
        /// Model name available on the Ollama instance.
        model: String,
    },
}

/// Per-token pricing for an LLM model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Cost per input token in USD.
    pub input_cost_per_token: f64,
    /// Cost per output token in USD.
    pub output_cost_per_token: f64,
}

impl ModelPricing {
    /// Return known pricing for a model name, falling back to Sonnet rates.
    pub fn default_for_model(model: &str) -> Self {
        let lower = model.to_lowercase();
        if lower.contains("opus") {
            // Claude Opus: $15/$75 per 1M tokens
            Self {
                input_cost_per_token: 0.000015,
                output_cost_per_token: 0.000075,
            }
        } else if lower.contains("haiku") {
            // Claude Haiku: $0.25/$1.25 per 1M tokens
            Self {
                input_cost_per_token: 0.00000025,
                output_cost_per_token: 0.00000125,
            }
        } else if lower.contains("gpt-4o-mini") {
            // GPT-4o mini: $0.15/$0.60 per 1M tokens
            Self {
                input_cost_per_token: 0.00000015,
                output_cost_per_token: 0.0000006,
            }
        } else if lower.contains("gpt-4o") {
            // GPT-4o: $2.50/$10 per 1M tokens
            Self {
                input_cost_per_token: 0.0000025,
                output_cost_per_token: 0.00001,
            }
        } else {
            // Default: Claude Sonnet rates ($3/$15 per 1M tokens)
            Self {
                input_cost_per_token: 0.000003,
                output_cost_per_token: 0.000015,
            }
        }
    }
}

impl ProviderConfig {
    /// Return the model name from the configuration.
    pub fn model_name(&self) -> &str {
        match self {
            ProviderConfig::Claude { model, .. } => model,
            ProviderConfig::OpenAI { model, .. } => model,
            ProviderConfig::Ollama { model, .. } => model,
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::Claude {
            api_key_ref: "claude_api_key".into(),
            model: "claude-sonnet-4-20250514".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_claude() {
        let p = ProviderConfig::default();
        assert!(matches!(p, ProviderConfig::Claude { .. }));
    }

    #[test]
    fn toml_roundtrip() {
        let p = ProviderConfig::OpenAI {
            api_key_ref: "openai_key".into(),
            model: "gpt-4o".into(),
        };
        let toml_str = toml::to_string(&p).unwrap();
        let parsed: ProviderConfig = toml::from_str(&toml_str).unwrap();
        assert!(matches!(parsed, ProviderConfig::OpenAI { .. }));
    }

    #[test]
    fn model_pricing_sonnet_default() {
        let p = ModelPricing::default_for_model("claude-sonnet-4-20250514");
        assert!((p.input_cost_per_token - 0.000003).abs() < 1e-10);
        assert!((p.output_cost_per_token - 0.000015).abs() < 1e-10);
    }

    #[test]
    fn model_pricing_opus() {
        let p = ModelPricing::default_for_model("claude-opus-4-20250514");
        assert!((p.input_cost_per_token - 0.000015).abs() < 1e-10);
    }

    #[test]
    fn model_pricing_haiku() {
        let p = ModelPricing::default_for_model("claude-haiku-3-5-20240307");
        assert!((p.input_cost_per_token - 0.00000025).abs() < 1e-10);
    }

    #[test]
    fn model_pricing_gpt4o() {
        let p = ModelPricing::default_for_model("gpt-4o");
        assert!((p.input_cost_per_token - 0.0000025).abs() < 1e-10);
    }

    #[test]
    fn model_pricing_unknown_falls_back_to_sonnet() {
        let p = ModelPricing::default_for_model("some-unknown-model");
        assert!((p.input_cost_per_token - 0.000003).abs() < 1e-10);
    }

    #[test]
    fn provider_config_model_name() {
        let c = ProviderConfig::default();
        assert!(c.model_name().contains("claude"));
    }

    #[test]
    fn ollama_has_no_api_key() {
        let p = ProviderConfig::Ollama {
            base_url: "http://localhost:11434".into(),
            model: "llama3".into(),
        };
        let toml_str = toml::to_string(&p).unwrap();
        assert!(!toml_str.contains("api_key_ref"));
    }
}
