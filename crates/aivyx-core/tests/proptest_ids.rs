use aivyx_core::{AgentId, AivyxError};
use proptest::prelude::*;

proptest! {
    /// Any valid v4 UUID string should parse into an AgentId and round-trip
    /// through Display back to the same string.
    #[test]
    fn agent_id_roundtrip(
        id in "[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    ) {
        let parsed: AgentId = id.parse().unwrap();
        let displayed = parsed.to_string();
        prop_assert_eq!(displayed, id);
    }

    /// AivyxError::Display should never panic for arbitrary string payloads.
    #[test]
    fn error_display_never_panics(s in "\\PC{0,500}") {
        // Test several string-carrying variants to ensure Display is robust.
        let errors = vec![
            AivyxError::Crypto(s.clone()),
            AivyxError::Config(s.clone()),
            AivyxError::Storage(s.clone()),
            AivyxError::LlmProvider(s.clone()),
            AivyxError::Http(s.clone()),
            AivyxError::RateLimit(s.clone()),
            AivyxError::Agent(s.clone()),
            AivyxError::Embedding(s.clone()),
            AivyxError::Memory(s.clone()),
            AivyxError::Task(s.clone()),
            AivyxError::Other(s.clone()),
            AivyxError::CapabilityDenied(s.clone()),
            AivyxError::CapabilityNotFound(s.clone()),
            AivyxError::AuditIntegrity(s.clone()),
            AivyxError::NotInitialized(s.clone()),
            AivyxError::TomlSer(s.clone()),
            AivyxError::TomlDe(s.clone()),
            AivyxError::Scheduler(s.clone()),
        ];

        for err in &errors {
            // Just call Display -- if it panics, proptest catches it.
            let _msg = err.to_string();
            // No assertion on content — we just verify it doesn't panic.
        }
    }

    /// The Context variant should always include its message in Display output.
    #[test]
    fn context_error_preserves_message(msg in "[a-zA-Z0-9 ]{1,100}") {
        let inner = AivyxError::Other("inner".into());
        let ctx = AivyxError::Context {
            message: msg.clone(),
            source: Box::new(inner),
        };
        let displayed = ctx.to_string();
        prop_assert_eq!(displayed, msg);
    }
}
