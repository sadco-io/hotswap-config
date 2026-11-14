//! Subscriber-based notifications for configuration changes.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

type SubscriberCallback = Arc<dyn Fn() + Send + Sync>;
type CountChangeHook = Arc<dyn Fn(usize) + Send + Sync>;

/// Handle for a subscription that can be dropped to unsubscribe.
///
/// When the handle is dropped, the subscription is synchronously removed
/// without requiring an active Tokio runtime.
pub struct SubscriptionHandle {
    id: usize,
    registry: Arc<Mutex<SubscriberRegistryInner>>,
}

impl fmt::Debug for SubscriptionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionHandle")
            .field("id", &self.id)
            .finish()
    }
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        let (hook, new_count) = {
            let mut inner = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            let before = inner.subscribers.len();
            inner.subscribers.retain(|(sub_id, _)| *sub_id != self.id);
            let after = inner.subscribers.len();
            let hook = if after != before {
                inner.on_count_change.clone()
            } else {
                None
            };
            (hook, after)
        };

        if let Some(hook) = hook {
            hook(new_count);
        }
    }
}

/// Internal subscriber registry state.
struct SubscriberRegistryInner {
    subscribers: Vec<(usize, SubscriberCallback)>,
    next_id: usize,
    on_count_change: Option<CountChangeHook>,
}

/// Registry for managing configuration change subscribers.
///
/// Allows code to register callbacks that are invoked whenever the
/// configuration is updated.
///
/// # Examples
///
/// ```rust,no_run
/// use hotswap_config::notify::SubscriberRegistry;
///
/// # async fn example() {
/// let registry = SubscriberRegistry::new();
///
/// let handle = registry.subscribe(|| {
///     println!("Config changed!");
/// }).await;
///
/// // Notify all subscribers
/// registry.notify_all().await;
///
/// // Unsubscribe synchronously by dropping the handle
/// drop(handle);
/// # }
/// ```
pub struct SubscriberRegistry {
    inner: Arc<Mutex<SubscriberRegistryInner>>,
}

impl fmt::Debug for SubscriberRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (count, next_id) = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            (inner.subscribers.len(), inner.next_id)
        };
        f.debug_struct("SubscriberRegistry")
            .field("subscriber_count", &count)
            .field("next_id", &next_id)
            .finish()
    }
}

impl SubscriberRegistry {
    /// Create a new subscriber registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SubscriberRegistryInner {
                subscribers: Vec::new(),
                next_id: 0,
                on_count_change: None,
            })),
        }
    }

    /// Create a subscriber registry with a count-change observer hook.
    #[allow(dead_code)]
    pub(crate) fn with_on_count_change<F>(hook: F) -> Self
    where
        F: Fn(usize) + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(Mutex::new(SubscriberRegistryInner {
                subscribers: Vec::new(),
                next_id: 0,
                on_count_change: Some(Arc::new(hook)),
            })),
        }
    }

    /// Subscribe to configuration changes.
    ///
    /// The provided callback will be invoked whenever the configuration
    /// is updated. Returns a handle that can be dropped to unsubscribe.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to call when config changes
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hotswap_config::notify::SubscriberRegistry;
    /// # async fn example() {
    /// let registry = SubscriberRegistry::new();
    ///
    /// let handle = registry.subscribe(|| {
    ///     println!("Configuration updated!");
    /// }).await;
    ///
    /// // Later, unsubscribe
    /// drop(handle);
    /// # }
    /// ```
    pub async fn subscribe<F>(&self, callback: F) -> SubscriptionHandle
    where
        F: Fn() + Send + Sync + 'static,
    {
        let (id, hook, new_count) = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let id = inner.next_id;
            inner.next_id += 1;
            inner.subscribers.push((id, Arc::new(callback)));
            let count = inner.subscribers.len();
            (id, inner.on_count_change.clone(), count)
        };

        if let Some(hook) = hook {
            hook(new_count);
        }

        SubscriptionHandle {
            id,
            registry: Arc::clone(&self.inner),
        }
    }

    /// Notify all subscribers of a configuration change.
    ///
    /// This calls all registered callbacks in the order they were subscribed.
    /// Each callback is isolated with `catch_unwind` so a panicking subscriber
    /// cannot prevent remaining subscribers from running.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hotswap_config::notify::SubscriberRegistry;
    /// # async fn example() {
    /// let registry = SubscriberRegistry::new();
    ///
    /// registry.subscribe(|| println!("Subscriber 1")).await;
    /// registry.subscribe(|| println!("Subscriber 2")).await;
    ///
    /// // Notify all subscribers
    /// registry.notify_all().await;
    /// # }
    /// ```
    pub async fn notify_all(&self) {
        let callbacks: Vec<SubscriberCallback> = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner
                .subscribers
                .iter()
                .map(|(_, cb)| Arc::clone(cb))
                .collect()
        };

        for callback in callbacks {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                callback();
            }));
        }
    }

    /// Get the number of active subscribers.
    pub async fn subscriber_count(&self) -> usize {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.subscribers.len()
    }
}

impl Default for SubscriberRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SubscriberRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_subscribe_and_notify() {
        let registry = SubscriberRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter_clone = Arc::clone(&counter);
        let handle = registry
            .subscribe(move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        assert!(format!("{:?}", registry).contains("SubscriberRegistry"));
        assert!(format!("{:?}", handle).contains("SubscriptionHandle"));

        registry.notify_all().await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        registry.notify_all().await;
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let registry = SubscriberRegistry::new();
        let counter1 = Arc::new(AtomicUsize::new(0));
        let counter2 = Arc::new(AtomicUsize::new(0));

        let counter1_clone = Arc::clone(&counter1);
        let _handle1 = registry
            .subscribe(move || {
                counter1_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        let counter2_clone = Arc::clone(&counter2);
        let _handle2 = registry
            .subscribe(move || {
                counter2_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        registry.notify_all().await;
        assert_eq!(counter1.load(Ordering::SeqCst), 1);
        assert_eq!(counter2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_unsubscribe_immediate_without_sleep() {
        let registry = SubscriberRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let counter_clone = Arc::clone(&counter);
        let handle = registry
            .subscribe(move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        registry.notify_all().await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Unsubscribe by dropping handle — takes effect synchronously with zero sleep!
        drop(handle);
        assert_eq!(registry.subscriber_count().await, 0);

        registry.notify_all().await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_drop_outside_tokio_runtime_does_not_panic() {
        let handle = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let registry = SubscriberRegistry::new();
                registry.subscribe(|| {}).await
            })
        };
        // Runtime has shut down completely; dropping `handle` must not panic!
        drop(handle);
    }

    #[tokio::test]
    async fn test_panicking_subscriber_isolated() {
        let registry = SubscriberRegistry::new();
        let after_panic_counter = Arc::new(AtomicUsize::new(0));
        let after_clone = Arc::clone(&after_panic_counter);

        let _bad = registry
            .subscribe(|| {
                panic!("boom inside subscriber");
            })
            .await;

        let _good = registry
            .subscribe(move || {
                after_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        registry.notify_all().await;
        assert_eq!(after_panic_counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_count_change_hook() {
        let observed = Arc::new(AtomicUsize::new(999));
        let observed_clone = Arc::clone(&observed);
        let registry = SubscriberRegistry::with_on_count_change(move |count| {
            observed_clone.store(count, Ordering::SeqCst);
        });

        let h1 = registry.subscribe(|| {}).await;
        assert_eq!(observed.load(Ordering::SeqCst), 1);

        let h2 = registry.subscribe(|| {}).await;
        assert_eq!(observed.load(Ordering::SeqCst), 2);

        drop(h1);
        assert_eq!(observed.load(Ordering::SeqCst), 1);

        drop(h2);
        assert_eq!(observed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_subscriber_count() {
        let registry = SubscriberRegistry::new();
        assert_eq!(registry.subscriber_count().await, 0);

        let handle1 = registry.subscribe(|| {}).await;
        assert_eq!(registry.subscriber_count().await, 1);

        let _handle2 = registry.subscribe(|| {}).await;
        assert_eq!(registry.subscriber_count().await, 2);

        drop(handle1);
        assert_eq!(registry.subscriber_count().await, 1);
    }

    #[tokio::test]
    async fn test_clone_registry() {
        let registry = SubscriberRegistry::new();
        let registry2 = registry.clone();

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        let _handle = registry
            .subscribe(move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        registry2.notify_all().await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
