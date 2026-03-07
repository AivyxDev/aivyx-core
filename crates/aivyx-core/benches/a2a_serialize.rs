//! Benchmarks for A2A JSON-RPC serialization — hot path on every /a2a request.

use aivyx_core::a2a::{
    A2aArtifact, A2aMessage, A2aPart, A2aRole, A2aTask, A2aTaskState, A2aTaskStatus,
    AgentCapabilities, AgentCard, JsonRpcRequest, JsonRpcResponse, TaskStatusUpdateEvent,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn sample_jsonrpc_request() -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        method: "tasks/send".into(),
        params: serde_json::json!({
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "Summarize the quarterly report and highlight key metrics."}]
            }
        }),
        id: serde_json::json!(42),
    }
}

fn sample_a2a_task() -> A2aTask {
    A2aTask {
        id: "task-abc-123".into(),
        status: A2aTaskStatus {
            state: A2aTaskState::Completed,
            message: Some(A2aMessage {
                role: A2aRole::Agent,
                parts: vec![A2aPart::Text {
                    text: "The quarterly report shows revenue growth of 15%.".into(),
                }],
            }),
            timestamp: "2025-01-15T10:30:00Z".into(),
        },
        history: Some(vec![
            A2aMessage {
                role: A2aRole::User,
                parts: vec![A2aPart::Text {
                    text: "Summarize the quarterly report.".into(),
                }],
            },
            A2aMessage {
                role: A2aRole::Agent,
                parts: vec![A2aPart::Text {
                    text: "The quarterly report shows revenue growth of 15%.".into(),
                }],
            },
        ]),
        artifacts: Some(vec![A2aArtifact {
            name: Some("summary.txt".into()),
            parts: vec![A2aPart::Text {
                text: "Key metrics: Revenue +15%, Users +22%, Churn -3%".into(),
            }],
        }]),
        metadata: None,
    }
}

fn sample_agent_card() -> AgentCard {
    AgentCard {
        name: "Aivyx Engine".into(),
        description: "AI agent orchestration platform".into(),
        url: "https://api.aivyx.dev".into(),
        version: "0.1.0".into(),
        capabilities: AgentCapabilities {
            streaming: true,
            push_notifications: true,
        },
        skills: vec![],
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into()],
        authentication: None,
    }
}

fn bench_jsonrpc_request(c: &mut Criterion) {
    let mut group = c.benchmark_group("jsonrpc_request");
    let req = sample_jsonrpc_request();
    let json = serde_json::to_string(&req).unwrap();

    group.bench_function("serialize", |b| {
        b.iter(|| black_box(serde_json::to_string(&req).unwrap()));
    });
    group.bench_function("deserialize", |b| {
        b.iter(|| black_box(serde_json::from_str::<JsonRpcRequest>(&json).unwrap()));
    });
    group.finish();
}

fn bench_jsonrpc_response(c: &mut Criterion) {
    let mut group = c.benchmark_group("jsonrpc_response");

    let success = JsonRpcResponse::success(serde_json::json!(1), serde_json::json!({"status": "ok"}));
    let success_json = serde_json::to_string(&success).unwrap();

    group.bench_function("success_serialize", |b| {
        b.iter(|| black_box(serde_json::to_string(&success).unwrap()));
    });
    group.bench_function("success_deserialize", |b| {
        b.iter(|| {
            black_box(serde_json::from_str::<JsonRpcResponse>(&success_json).unwrap())
        });
    });
    group.finish();
}

fn bench_a2a_task(c: &mut Criterion) {
    let mut group = c.benchmark_group("a2a_task");
    let task = sample_a2a_task();
    let json = serde_json::to_string(&task).unwrap();

    group.bench_function("serialize", |b| {
        b.iter(|| black_box(serde_json::to_string(&task).unwrap()));
    });
    group.bench_function("deserialize", |b| {
        b.iter(|| black_box(serde_json::from_str::<A2aTask>(&json).unwrap()));
    });
    group.finish();
}

fn bench_agent_card(c: &mut Criterion) {
    let mut group = c.benchmark_group("agent_card");
    let card = sample_agent_card();
    let json = serde_json::to_string(&card).unwrap();

    group.bench_function("serialize", |b| {
        b.iter(|| black_box(serde_json::to_string(&card).unwrap()));
    });
    group.bench_function("deserialize", |b| {
        b.iter(|| black_box(serde_json::from_str::<AgentCard>(&json).unwrap()));
    });
    group.finish();
}

fn bench_sse_event(c: &mut Criterion) {
    let event = TaskStatusUpdateEvent {
        id: "task-abc-123".into(),
        status: A2aTaskStatus {
            state: A2aTaskState::Working,
            message: Some(A2aMessage {
                role: A2aRole::Agent,
                parts: vec![A2aPart::Text {
                    text: "Processing step 3 of 5...".into(),
                }],
            }),
            timestamp: "2025-01-15T10:30:05Z".into(),
        },
        is_final: false,
    };
    let json = serde_json::to_string(&event).unwrap();

    c.bench_function("sse_event_serialize", |b| {
        b.iter(|| black_box(serde_json::to_string(&event).unwrap()));
    });
    c.bench_function("sse_event_deserialize", |b| {
        b.iter(|| {
            black_box(serde_json::from_str::<TaskStatusUpdateEvent>(&json).unwrap())
        });
    });
}

criterion_group!(
    benches,
    bench_jsonrpc_request,
    bench_jsonrpc_response,
    bench_a2a_task,
    bench_agent_card,
    bench_sse_event,
);
criterion_main!(benches);
