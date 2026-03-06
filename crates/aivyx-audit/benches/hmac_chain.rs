use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_core::{AgentId, AutonomyTier};
use std::path::PathBuf;

fn temp_log_path() -> PathBuf {
    let id = uuid::Uuid::new_v4();
    std::env::temp_dir().join(format!("aivyx-bench-audit-{id}.jsonl"))
}

fn make_event() -> AuditEvent {
    AuditEvent::AgentCreated {
        agent_id: AgentId::new(),
        autonomy_tier: AutonomyTier::Trust,
    }
}

fn bench_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("hmac_chain_append");
    let key_bytes = [0x42u8; 32];

    for count in [100, 1000] {
        group.bench_with_input(BenchmarkId::new("append", count), &count, |b, &count| {
            b.iter(|| {
                let path = temp_log_path();
                let log = AuditLog::new(&path, &key_bytes);

                for _ in 0..count {
                    log.append(make_event()).expect("append failed");
                }

                drop(log);
                let _ = std::fs::remove_file(&path);
            });
        });
    }

    group.finish();
}

fn bench_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("hmac_chain_verify");
    let key_bytes = [0x42u8; 32];

    for count in [100, 1000] {
        // Pre-populate a log file for verification
        let path = temp_log_path();
        let log = AuditLog::new(&path, &key_bytes);
        for _ in 0..count {
            log.append(make_event()).expect("append failed");
        }

        group.bench_with_input(BenchmarkId::new("verify", count), &count, |b, &_count| {
            b.iter(|| {
                let verify_log = AuditLog::new(&path, &key_bytes);
                let _result = verify_log.verify().expect("verify failed");
            });
        });

        // Clean up after benchmark
        let _ = std::fs::remove_file(&path);
    }

    group.finish();
}

criterion_group!(benches, bench_append, bench_verify);
criterion_main!(benches);
