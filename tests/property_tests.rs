//! Property-based tests using `proptest` for recursive configuration deep-merging,
//! dotted map key preservation, FNV-1a consistent rollout hashing, and rollout
//! percentage clamping / `u8` overflow safety.

use hotswap_config::core::HotswapConfig;
use hotswap_config::error::Result;
use hotswap_config::sources::ConfigSource;
use proptest::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;

fn run_async<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(fut)
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct MergedSchema {
    section: HashMap<String, i64>,
    labels: HashMap<String, String>,
}

struct StaticMapSource {
    name: &'static str,
    priority: i32,
    data: HashMap<String, config::Value>,
}

impl ConfigSource for StaticMapSource {
    fn load(&self) -> Result<HashMap<String, config::Value>> {
        Ok(self.data.clone())
    }

    fn name(&self) -> String {
        self.name.to_string()
    }

    fn priority(&self) -> i32 {
        self.priority
    }
}

fn to_int_table(entries: &HashMap<String, i64>) -> config::Value {
    let table: HashMap<String, config::Value> = entries
        .iter()
        .map(|(k, v)| (k.clone(), config::Value::from(*v)))
        .collect();
    config::Value::new(None, config::ValueKind::Table(table))
}

fn to_string_table(entries: &HashMap<String, String>) -> config::Value {
    let table: HashMap<String, config::Value> = entries
        .iter()
        .map(|(k, v)| (k.clone(), config::Value::from(v.as_str())))
        .collect();
    config::Value::new(None, config::ValueKind::Table(table))
}

proptest! {
    /// Verifies that higher-priority sources override overlapping nested keys while
    /// preserving non-overlapping sibling keys from lower-priority sources AND preserving
    /// literal `.` characters inside user `HashMap` keys.
    #[test]
    fn prop_recursive_deep_merge_precedence_and_dotted_keys(
        base_section in prop::collection::hash_map("[a-z]{1,8}", any::<i64>(), 1..12),
        override_section in prop::collection::hash_map("[a-z]{1,8}", any::<i64>(), 0..12),
        base_labels in prop::collection::hash_map("[a-z]{1,4}(\\.[a-z]{1,4}){0,2}", "[a-zA-Z0-9_-]{0,12}", 1..10),
        override_labels in prop::collection::hash_map("[a-z]{1,4}(\\.[a-z]{1,4}){0,2}", "[a-zA-Z0-9_-]{0,12}", 0..10),
    ) {
        let mut low_root = HashMap::new();
        low_root.insert("section".to_string(), to_int_table(&base_section));
        low_root.insert("labels".to_string(), to_string_table(&base_labels));

        let mut high_root = HashMap::new();
        if !override_section.is_empty() {
            high_root.insert("section".to_string(), to_int_table(&override_section));
        }
        if !override_labels.is_empty() {
            high_root.insert("labels".to_string(), to_string_table(&override_labels));
        }

        // Register in reverse priority order to verify priority sorting as well as deep-merge
        let config = run_async(
            HotswapConfig::builder()
                .with_source(StaticMapSource {
                    name: "high",
                    priority: 200,
                    data: high_root,
                })
                .with_source(StaticMapSource {
                    name: "low",
                    priority: 100,
                    data: low_root,
                })
                .build::<MergedSchema>(),
        )
        .expect("deep-merged schema should deserialize");
        let loaded = config.get();

        let mut expected_section = base_section;
        for (k, v) in override_section {
            expected_section.insert(k, v);
        }

        let mut expected_labels = base_labels;
        for (k, v) in override_labels {
            expected_labels.insert(k, v);
        }

        prop_assert_eq!(&loaded.section, &expected_section);
        prop_assert_eq!(&loaded.labels, &expected_labels);
    }
}

#[cfg(feature = "gradual-rollout")]
mod rollout_props {
    use super::*;
    use hotswap_config::features::GradualRollout;
    use std::sync::Arc;

    proptest! {
        /// FNV-1a consistent hashing must assign any arbitrary UTF-8 key deterministically
        /// across repeated reads and across distinct `GradualRollout` instances.
        #[test]
        fn prop_fnv1a_rollout_assignment_is_deterministic(
            key in ".*",
            percentage in 0u8..=100u8,
        ) {
            let r1 = GradualRollout::new(Arc::new(0u32));
            let r2 = GradualRollout::new(Arc::new(0u32));
            run_async(async {
                r1.set_canary(Arc::new(1u32), percentage).await;
                r2.set_canary(Arc::new(1u32), percentage).await;
            });

            let first = *r1.get_sync(Some(&key));
            let second = *r1.get_sync(Some(&key));
            let other_instance = *r2.get_sync(Some(&key));

            prop_assert_eq!(first, second);
            prop_assert_eq!(first, other_instance);

            if percentage == 0 {
                prop_assert_eq!(first, 0u32);
                prop_assert_eq!(*r1.get_sync(None), 0u32);
            } else if percentage == 100 {
                prop_assert_eq!(first, 1u32);
                prop_assert_eq!(*r1.get_sync(None), 1u32);
            }
        }

        /// `set_canary` and arbitrary sequences of `increase_percentage` deltas (including
        /// `u8::MAX`) must never overflow `u8`, must stay monotonically non-decreasing,
        /// and must always clamp to `0..=100`.
        #[test]
        fn prop_rollout_percentage_clamping_and_overflow_safety(
            initial in any::<u8>(),
            deltas in prop::collection::vec(any::<u8>(), 0..25),
            key in ".*",
        ) {
            let rollout = GradualRollout::new(Arc::new("stable"));
            let mut prev = run_async(async {
                rollout.set_canary(Arc::new("canary"), initial).await;
                rollout.get_percentage().await
            });
            prop_assert_eq!(prev, initial.min(100));
            prop_assert!(prev <= 100);

            if initial >= 100 {
                prop_assert_eq!(*rollout.get_sync(Some(&key)), "canary");
                prop_assert_eq!(*rollout.get_sync(None), "canary");
            }

            for delta in deltas {
                let (updated, current) = run_async(async {
                    let u = rollout.increase_percentage(delta).await;
                    let c = rollout.get_percentage().await;
                    (u, c)
                });
                prop_assert!(updated >= prev);
                prop_assert!(updated <= 100);
                prop_assert_eq!(updated, prev.saturating_add(delta).min(100));
                prop_assert_eq!(current, updated);
                prev = updated;
            }
        }
    }
}
