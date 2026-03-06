use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use aivyx_crypto::{EncryptedStore, MasterKey};
use std::path::PathBuf;

fn temp_store_path() -> PathBuf {
    let id = uuid::Uuid::new_v4();
    std::env::temp_dir().join(format!("aivyx-bench-crypto-{id}"))
}

fn bench_put_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("encrypt_decrypt");

    for size in [1_024, 10_240, 102_400] {
        let label = match size {
            1_024 => "1KB",
            10_240 => "10KB",
            102_400 => "100KB",
            _ => unreachable!(),
        };

        let data = vec![0xABu8; size];
        let master_key = MasterKey::generate();

        group.bench_with_input(BenchmarkId::new("put_get", label), &data, |b, data| {
            let path = temp_store_path();
            let store = EncryptedStore::open(&path).expect("open store");

            b.iter(|| {
                store
                    .put("bench-key", data, &master_key)
                    .expect("put failed");
                let _val = store.get("bench-key", &master_key).expect("get failed");
            });

            // Clean up
            drop(store);
            let _ = std::fs::remove_dir_all(&path);
        });
    }

    group.finish();
}

criterion_group!(benches, bench_put_get);
criterion_main!(benches);
