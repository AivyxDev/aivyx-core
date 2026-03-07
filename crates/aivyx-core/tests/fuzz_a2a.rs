//! Property-based fuzzing for A2A protocol types.
//!
//! Ensures that parsing arbitrary strings never panics and that
//! A2A type invariants hold under random input.

use aivyx_core::a2a::{
    A2aTask, A2aTaskState, AgentCard, JsonRpcRequest, JsonRpcResponse, PushNotificationConfig,
    TaskStatusUpdateEvent,
};
use proptest::prelude::*;

proptest! {
    // --- Parse-don't-panic: arbitrary strings must never cause a panic ---

    #[test]
    fn fuzz_json_rpc_request_parse(s in "\\PC*") {
        let _ = serde_json::from_str::<JsonRpcRequest>(&s);
    }

    #[test]
    fn fuzz_json_rpc_response_parse(s in "\\PC*") {
        let _ = serde_json::from_str::<JsonRpcResponse>(&s);
    }

    #[test]
    fn fuzz_agent_card_parse(s in "\\PC*") {
        let _ = serde_json::from_str::<AgentCard>(&s);
    }

    #[test]
    fn fuzz_a2a_task_parse(s in "\\PC*") {
        let _ = serde_json::from_str::<A2aTask>(&s);
    }

    #[test]
    fn fuzz_task_status_update_event_parse(s in "\\PC*") {
        let _ = serde_json::from_str::<TaskStatusUpdateEvent>(&s);
    }

    #[test]
    fn fuzz_push_notification_config_parse(s in "\\PC*") {
        let _ = serde_json::from_str::<PushNotificationConfig>(&s);
    }

    // --- A2aTaskState: all 6 variants roundtrip correctly ---

    #[test]
    fn a2a_task_state_roundtrip(idx in 0usize..6usize) {
        let states = [
            A2aTaskState::Submitted,
            A2aTaskState::Working,
            A2aTaskState::InputRequired,
            A2aTaskState::Completed,
            A2aTaskState::Failed,
            A2aTaskState::Canceled,
        ];
        let state = &states[idx];
        let json = serde_json::to_string(state).unwrap();
        let restored: A2aTaskState = serde_json::from_str(&json).unwrap();
        // Compare via serialized form since A2aTaskState may not impl PartialEq
        let restored_json = serde_json::to_string(&restored).unwrap();
        prop_assert_eq!(&json, &restored_json);
    }

    // --- Terminal states are distinct ---

    #[test]
    fn terminal_states_are_distinct(_ in 0u8..10u8) {
        let completed = serde_json::to_string(&A2aTaskState::Completed).unwrap();
        let failed = serde_json::to_string(&A2aTaskState::Failed).unwrap();
        let canceled = serde_json::to_string(&A2aTaskState::Canceled).unwrap();
        prop_assert_ne!(&completed, &failed);
        prop_assert_ne!(&completed, &canceled);
        prop_assert_ne!(&failed, &canceled);
    }

    // --- JsonRpcResponse: success has result, no error ---

    #[test]
    fn json_rpc_success_has_result_not_error(val in "[a-zA-Z0-9 ]{1,50}") {
        let resp = JsonRpcResponse::success(
            serde_json::json!(1),
            serde_json::json!(val),
        );
        let json = serde_json::to_value(&resp).unwrap();
        prop_assert!(json.get("error").map_or(true, |v| v.is_null()));
        prop_assert!(!json["result"].is_null());
    }

    // --- JsonRpcResponse: error has error, no result ---

    #[test]
    fn json_rpc_error_has_error_not_result(
        code in -32099i32..-32000i32,
        msg in "[a-zA-Z0-9 ]{1,50}"
    ) {
        let resp = JsonRpcResponse::error(serde_json::json!(1), code, msg);
        let json = serde_json::to_value(&resp).unwrap();
        prop_assert!(json.get("result").map_or(true, |v| v.is_null()));
        prop_assert!(!json["error"].is_null());
    }
}
