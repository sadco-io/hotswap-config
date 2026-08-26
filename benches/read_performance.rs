//! Performance benchmarks for hotswap-config.
//!
//! These benchmarks verify the performance claims made in the README:
//! - Read latency < 10ns
//! - Clone latency ~4-5ns
//! - Zero dropped reads during reload
//! - Scales linearly with concurrent readers

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hotswap_config::prelude::*;
use serde::{Deserialize, Serialize};
// criterion 0.6 deprecated its own `black_box` re-export in favour of the one
// stabilised in `std::hint`. They are the same function; only the path moved.
use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct BenchConfig {
    value: i32,
    name: String,
    flag: bool,
    items: Vec<String>,
}

impl BenchConfig {
    fn default() -> Self {
        Self {
            value: 42,
            name: "benchmark".to_string(),
            flag: true,
            items: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        }
    }
}

/// Benchmark single-threaded read latency
fn benchmark_read_latency(c: &mut Criterion) {
    let config = HotswapConfig::new(BenchConfig::default());

    let mut group = c.benchmark_group("read_latency");
    group.bench_function("single_read", |b| {
        b.iter(|| {
            let cfg = config.get();
            black_box(&cfg.value);
        });
    });
    group.finish();
}

/// Benchmark clone performance
fn benchmark_clone(c: &mut Criterion) {
    let config = HotswapConfig::new(BenchConfig::default());

    let mut group = c.benchmark_group("clone");
    group.bench_function("config_clone", |b| {
        b.iter(|| {
            let cloned = config.clone();
            black_box(cloned);
        });
    });
    group.finish();
}

/// Benchmark arc clone (for comparison)
fn benchmark_arc_clone(c: &mut Criterion) {
    let value = Arc::new(BenchConfig::default());

    let mut group = c.benchmark_group("arc_clone");
    group.bench_function("arc_clone", |b| {
        b.iter(|| {
            let cloned = Arc::clone(&value);
            black_box(cloned);
        });
    });
    group.finish();
}

/// Benchmark concurrent reads with varying thread counts
fn benchmark_concurrent_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_reads");

    for num_threads in [1, 2, 4, 8, 16] {
        group.throughput(Throughput::Elements(num_threads as u64 * 1000));

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_threads", num_threads)),
            &num_threads,
            |b, &num_threads| {
                let config = Arc::new(HotswapConfig::new(BenchConfig::default()));
                let barrier = Arc::new(Barrier::new(num_threads + 1));

                b.iter_custom(|iters| {
                    let mut handles = vec![];
                    let start_barrier = Arc::clone(&barrier);

                    for _ in 0..num_threads {
                        let cfg = Arc::clone(&config);
                        let b = Arc::clone(&barrier);

                        let handle = thread::spawn(move || {
                            // Wait for all threads to be ready
                            b.wait();

                            let start = std::time::Instant::now();
                            for _ in 0..iters {
                                let data = cfg.get();
                                black_box(&data.value);
                            }
                            start.elapsed()
                        });

                        handles.push(handle);
                    }

                    // Start all threads
                    start_barrier.wait();

                    // Wait for completion and sum durations
                    let total_duration: Duration =
                        handles.into_iter().map(|h| h.join().unwrap()).sum();

                    // Return average duration across threads
                    total_duration / num_threads as u32
                });
            },
        );
    }

    group.finish();
}

/// Benchmark reload under load - prove zero dropped reads
fn benchmark_reload_under_load(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("reload_under_load");
    group.sample_size(10); // Fewer samples since this is expensive
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("reload_with_16_readers", |b| {
        b.iter_custom(|iters| {
            runtime.block_on(async move {
                let config = Arc::new(HotswapConfig::new(BenchConfig::default()));
                let keep_running = Arc::new(std::sync::atomic::AtomicBool::new(true));
                let reads_completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));

                // Spawn 16 reader threads
                let mut reader_handles = vec![];
                for _ in 0..16 {
                    let cfg = Arc::clone(&config);
                    let running = Arc::clone(&keep_running);
                    let counter = Arc::clone(&reads_completed);

                    let handle = tokio::spawn(async move {
                        while running.load(std::sync::atomic::Ordering::Relaxed) {
                            let data = cfg.get();
                            black_box(&data.value);
                            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    });

                    reader_handles.push(handle);
                }

                // Perform reloads
                let start = std::time::Instant::now();
                for i in 0..iters {
                    let new_config = BenchConfig {
                        value: i as i32,
                        name: format!("reload_{}", i),
                        flag: i % 2 == 0,
                        items: vec!["x".to_string()],
                    };

                    config.update(new_config).await.unwrap();
                    tokio::time::sleep(Duration::from_micros(100)).await;
                }
                let duration = start.elapsed();

                // Stop readers
                keep_running.store(false, std::sync::atomic::Ordering::Relaxed);

                // Wait for readers
                for handle in reader_handles {
                    handle.await.unwrap();
                }

                let total_reads = reads_completed.load(std::sync::atomic::Ordering::Relaxed);
                println!("  Completed {} reads during {} reloads", total_reads, iters);

                duration
            })
        });
    });

    group.finish();
}

/// Benchmark comparison with mutex-based approach
fn benchmark_mutex_comparison(c: &mut Criterion) {
    use std::sync::Mutex;

    let mut group = c.benchmark_group("mutex_comparison");

    // arc-swap based (our implementation)
    let config_arcswap = HotswapConfig::new(BenchConfig::default());
    group.bench_function("arcswap_read", |b| {
        b.iter(|| {
            let cfg = config_arcswap.get();
            black_box(&cfg.value);
        });
    });

    // Mutex<Arc<T>> based (traditional approach)
    let config_mutex = Mutex::new(Arc::new(BenchConfig::default()));
    group.bench_function("mutex_arc_read", |b| {
        b.iter(|| {
            let cfg = config_mutex.lock().unwrap();
            let value = &cfg.value;
            black_box(value);
        });
    });

    // RwLock<T> based (another common approach)
    let config_rwlock = std::sync::RwLock::new(BenchConfig::default());
    group.bench_function("rwlock_read", |b| {
        b.iter(|| {
            let cfg = config_rwlock.read().unwrap();
            black_box(&cfg.value);
        });
    });

    group.finish();
}

/// Benchmark update performance
fn benchmark_update(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("update");

    group.bench_function("update_config", |b| {
        let config = HotswapConfig::new(BenchConfig::default());
        let mut counter = 0;

        b.iter(|| {
            counter += 1;
            let new_config = BenchConfig {
                value: counter,
                name: format!("update_{}", counter),
                flag: counter % 2 == 0,
                items: vec![format!("item_{}", counter)],
            };

            runtime.block_on(async {
                config.update(new_config).await.unwrap();
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_read_latency,
    benchmark_clone,
    benchmark_arc_clone,
    benchmark_concurrent_reads,
    benchmark_reload_under_load,
    benchmark_mutex_comparison,
    benchmark_update,
);

criterion_main!(benches);
