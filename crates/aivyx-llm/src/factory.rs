use aivyx_config::ProviderConfig;
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use secrecy::SecretString;

use crate::claude::ClaudeProvider;
use crate::ollama::OllamaProvider;
use crate::openai::OpenAIProvider;
use crate::provider::LlmProvider;

/// Create an LLM provider from configuration, resolving API keys from the encrypted store.
pub fn create_provider(
    config: &ProviderConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> Result<Box<dyn LlmProvider>> {
    match config {
        ProviderConfig::Claude { api_key_ref, model } => {
            let api_key = resolve_api_key(api_key_ref, store, master_key)?;
            Ok(Box::new(ClaudeProvider::new(api_key, model.clone())))
        }
        ProviderConfig::OpenAI { api_key_ref, model } => {
            let api_key = resolve_api_key(api_key_ref, store, master_key)?;
            Ok(Box::new(OpenAIProvider::new(api_key, model.clone())))
        }
        ProviderConfig::Ollama { base_url, model } => Ok(Box::new(OllamaProvider::new(
            base_url.clone(),
            model.clone(),
        ))),
    }
}

fn resolve_api_key(
    key_ref: &str,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> Result<SecretString> {
    let bytes = store.get(key_ref, master_key)?.ok_or_else(|| {
        AivyxError::Config(format!(
            "API key '{key_ref}' not found in encrypted store. Run: aivyx secret set {key_ref}"
        ))
    })?;
    let key_str = String::from_utf8(bytes)
        .map_err(|e| AivyxError::Config(format!("API key is not valid UTF-8: {e}")))?;
    Ok(SecretString::from(key_str))
}
