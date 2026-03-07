//! Benchmarks for vector similarity search — a hot path during memory recall.

use aivyx_core::MemoryId;
use aivyx_memory::search::{VectorIndex, cosine_similarity};
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn make_index(n_vectors: usize, dims: usize) -> VectorIndex {
    let mut index = VectorIndex::new();
    for i in 0..n_vectors {
        let id = MemoryId::new();
        let v: Vec<f32> = (0..dims)
            .map(|d| ((i * dims + d) as f32 * 0.001).sin())
            .collect();
        index.upsert(id, v);
    }
    index
}

fn bench_vector_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_search");
    let dims = 1536; // OpenAI embedding dimension

    for n in [100, 1_000, 10_000] {
        let index = make_index(n, dims);
        let query: Vec<f32> = (0..dims).map(|i| (i as f32 * 0.002).cos()).collect();

        group.bench_with_input(BenchmarkId::new("top_k_5", n), &n, |b, _| {
            b.iter(|| black_box(index.search(&query, 5)));
        });
        group.bench_with_input(BenchmarkId::new("top_k_20", n), &n, |b, _| {
            b.iter(|| black_box(index.search(&query, 20)));
        });
    }
    group.finish();
}

fn bench_cosine_similarity(c: &mut Criterion) {
    let mut group = c.benchmark_group("cosine_similarity");

    for dims in [384, 1536, 3072] {
        let a: Vec<f32> = (0..dims).map(|i| (i as f32 * 0.01).sin()).collect();
        let b: Vec<f32> = (0..dims).map(|i| (i as f32 * 0.01).cos()).collect();

        group.bench_with_input(BenchmarkId::new("dims", dims), &dims, |bench, _| {
            bench.iter(|| black_box(cosine_similarity(&a, &b)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_vector_search, bench_cosine_similarity);
criterion_main!(benches);
