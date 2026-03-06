use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_core::{AgentId, AutonomyTier};
use chrono::Utc;
use proptest::prelude::*;

/// Strategy that generates a random AuditEvent from a handful of variants.
fn arb_audit_event() -> impl Strategy<Value = AuditEvent> {
    prop_oneof![
        Just(AuditEvent::SystemInit {
            timestamp: Utc::now(),
        }),
        Just(AuditEvent::AgentCreated {
            agent_id: AgentId::new(),
            autonomy_tier: AutonomyTier::Trust,
        }),
        Just(AuditEvent::AgentDestroyed {
            agent_id: AgentId::new(),
        }),
        Just(AuditEvent::MasterKeyRotated {
            timestamp: Utc::now(),
        }),
        ".*".prop_map(|reason| AuditEvent::HttpAuthFailed {
            remote_addr: "127.0.0.1:1234".into(),
            reason,
        }),
        (".*", ".*").prop_map(|(method, path)| AuditEvent::HttpRequestReceived {
            method,
            path,
            remote_addr: "10.0.0.1:5678".into(),
        }),
        ".*".prop_map(|goal| AuditEvent::TaskCreated {
            task_id: "test-task-id".into(),
            agent_name: "test-agent".into(),
            goal,
        }),
    ]
}

/// Helper to create a temp file path for audit logs.
fn tmp_path() -> std::path::PathBuf {
    let name = format!("aivyx_proptest_audit_{}.jsonl", uuid::Uuid::new_v4());
    std::env::temp_dir().join(name)
}

proptest! {
    /// Every AuditEvent variant should survive a JSON round-trip: serializing
    /// to JSON and deserializing back should produce a value whose
    /// re-serialized form matches the original JSON.
    #[test]
    fn audit_event_serde_roundtrip(event in arb_audit_event()) {
        let json = serde_json::to_string(&event).unwrap();
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&restored).unwrap();
        prop_assert_eq!(json, json2);
    }

    /// An HMAC-chained audit log of N random events should always verify
    /// successfully.
    #[test]
    fn hmac_chain_of_n_events_verifies(events in proptest::collection::vec(arb_audit_event(), 1..20)) {
        let path = tmp_path();
        let key = b"proptest-audit-hmac-key-32bytes!";
        let log = AuditLog::new(&path, key);

        for event in &events {
            log.append(event.clone()).unwrap();
        }

        let result = log.verify().unwrap();
        prop_assert!(result.valid, "HMAC chain should be valid");
        prop_assert_eq!(result.entries_checked, events.len() as u64);

        // Clean up
        std::fs::remove_file(&path).ok();
    }
}
