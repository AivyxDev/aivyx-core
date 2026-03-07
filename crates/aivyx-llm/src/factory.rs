use aivyx_config::speech::{SpeechConfig, SpeechProvider, TtsConfig, TtsProvider as TtsProviderConfig};
use aivyx_config::ProviderConfig;
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use secrecy::SecretString;

use crate::claude::ClaudeProvider;
use crate::ollama::OllamaProvider;
use crate::openai::OpenAIProvider;
use crate::provider::LlmProvider;
use crate::stt::SttProvider;
use crate::stt_ollama::OllamaSttProvider;
use crate::stt_openai::OpenAiSttProvider;
use crate::tts::TtsProvider;
use crate::tts_edge::EdgeTtsProvider;
use crate::tts_openai::OpenAiTtsProvider;

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

/// Create an STT provider from speech configuration, resolving API keys from the encrypted store.
pub fn create_stt_provider(
    config: &SpeechConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> Result<Box<dyn SttProvider>> {
    match &config.provider {
        SpeechProvider::OpenAi { api_key_ref } => {
            let api_key = resolve_api_key_string(api_key_ref, store, master_key)?;
            Ok(Box::new(OpenAiSttProvider::new(api_key, config.model.clone())))
        }
        SpeechProvider::Ollama { base_url } => {
            let base = base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".into());
            Ok(Box::new(OllamaSttProvider::new(base, config.model.clone())))
        }
    }
}

/// Create a TTS provider from TTS configuration, resolving API keys from the encrypted store.
pub fn create_tts_provider(
    config: &TtsConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> Result<Box<dyn TtsProvider>> {
    match &config.provider {
        TtsProviderConfig::OpenAi { api_key_ref } => {
            let api_key = resolve_api_key_string(api_key_ref, store, master_key)?;
            Ok(Box::new(OpenAiTtsProvider::new(api_key, config.model.clone())))
        }
        TtsProviderConfig::Edge => Ok(Box::new(EdgeTtsProvider::new())),
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

/// Resolve an API key as a plain String (for STT/TTS providers that don't use SecretString).
fn resolve_api_key_string(
    key_ref: &str,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> Result<String> {
    let bytes = store.get(key_ref, master_key)?.ok_or_else(|| {
        AivyxError::Config(format!(
            "API key '{key_ref}' not found in encrypted store. Run: aivyx secret set {key_ref}"
        ))
    })?;
    String::from_utf8(bytes)
        .map_err(|e| AivyxError::Config(format!("API key is not valid UTF-8: {e}")))
}
