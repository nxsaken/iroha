#![allow(missing_docs)]

mod apply_blocks;

use apply_blocks::WsvApplyBlocks;
use criterion::{criterion_group, criterion_main, Criterion};

fn apply_blocks(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime");
    let mut group = c.benchmark_group("apply_blocks");
    group.significance_level(0.1).sample_size(10);
    group.bench_function("apply_blocks", |b| {
        b.iter_batched_ref(
            || WsvApplyBlocks::setup(rt.handle()).expect("Failed to setup benchmark"),
            |bench| {
                WsvApplyBlocks::measure(bench).expect("Failed to execute benchmark");
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(wsv, apply_blocks);
criterion_main!(wsv);
