use aivyx_llm::ChatMessage;
use proptest::prelude::*;

proptest! {
    #[test]
    fn fuzz_chat_message_json(s in "\\PC*") {
        let _ = serde_json::from_str::<ChatMessage>(&s);
    }

    #[test]
    fn fuzz_estimate_tokens(content in "\\PC*") {
        let msg = ChatMessage::user(content);
        let _ = msg.estimate_tokens(); // must not panic
    }
}
