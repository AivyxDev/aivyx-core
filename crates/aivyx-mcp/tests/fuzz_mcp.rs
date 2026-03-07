//! Property-based fuzzing for MCP JSON-RPC protocol types.
//!
//! Ensures that parsing arbitrary strings never panics and that
//! MCP request constructors produce valid structures.

use aivyx_mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
use proptest::prelude::*;

proptest! {
    #[test]
    fn fuzz_mcp_json_rpc_response_parse(s in "\\PC*") {
        let _ = serde_json::from_str::<JsonRpcResponse>(&s);
    }

    #[test]
    fn mcp_request_roundtrip(
        id in 1u64..10000u64,
        method in "[a-z/]{1,30}"
    ) {
        let req = JsonRpcRequest::new(id, method.clone(), None);
        let json = serde_json::to_string(&req).unwrap();
        prop_assert!(json.contains("2.0"));
        prop_assert!(json.contains(&method));
    }

    #[test]
    fn mcp_notification_has_no_id(method in "[a-z/]{1,30}") {
        let notif = JsonRpcRequest::notification(method, None);
        let json = serde_json::to_value(&notif).unwrap();
        prop_assert!(json.get("id").is_none());
    }
}
