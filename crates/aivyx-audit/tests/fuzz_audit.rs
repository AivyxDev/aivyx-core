use proptest::prelude::*;

proptest! {
    #[test]
    fn fuzz_audit_entry_json(s in "\\PC*") {
        let _ = serde_json::from_str::<aivyx_audit::AuditEntry>(&s);
    }
}
