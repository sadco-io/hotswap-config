//! Integration tests for `HttpSource` across `current_thread` and `multi_thread` Tokio
//! runtimes as well as plain OS threads, against a real local `TcpListener` mock server.

use hotswap_config::error::ConfigError;
use hotswap_config::prelude::*;
use hotswap_config::sources::{ConfigSource, HttpSource};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct RemoteAppConfig {
    service_name: String,
    port: u16,
    max_quota_bytes: u64,
}

#[derive(Clone)]
enum MockReply {
    Json { status: u16, body: String },
    DropConnection,
}

struct ServerState {
    replies: VecDeque<MockReply>,
    fallback: MockReply,
    auth_headers: Vec<Option<String>>,
    request_count: usize,
}

struct MockHttpServer {
    url: String,
    state: Arc<Mutex<ServerState>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl MockHttpServer {
    async fn start(initial_replies: Vec<MockReply>, fallback: MockReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test listener");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://{}/api/config", addr);

        let state = Arc::new(Mutex::new(ServerState {
            replies: initial_replies.into(),
            fallback,
            auth_headers: Vec::new(),
            request_count: 0,
        }));
        let state_clone = Arc::clone(&state);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accept_res = listener.accept() => {
                        let Ok((mut stream, _)) = accept_res else {
                            break;
                        };
                        let conn_state = Arc::clone(&state_clone);
                        tokio::spawn(async move {
                            // Support HTTP/1.1 keep-alive across multiple sequential requests on
                            // the same socket so connection pooling edge cases are exercised.
                            loop {
                                let mut buf = Vec::with_capacity(1024);
                                let mut tmp = [0u8; 512];
                                let header_end;

                                loop {
                                    match stream.read(&mut tmp).await {
                                        Ok(0) | Err(_) => return,
                                        Ok(n) => {
                                            buf.extend_from_slice(&tmp[..n]);
                                            if let Some(pos) = buf
                                                .windows(4)
                                                .position(|w| w == b"\r\n\r\n")
                                            {
                                                header_end = pos + 4;
                                                break;
                                            }
                                            if buf.len() > 16 * 1024 {
                                                return;
                                            }
                                        }
                                    }
                                }

                                let headers_str = String::from_utf8_lossy(&buf[..header_end]);
                                let auth_header = headers_str.lines().find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    if name.eq_ignore_ascii_case("authorization") {
                                        Some(value.trim().to_string())
                                    } else {
                                        None
                                    }
                                });

                                let reply = {
                                    let mut guard = conn_state.lock().unwrap();
                                    guard.request_count += 1;
                                    guard.auth_headers.push(auth_header);
                                    guard
                                        .replies
                                        .pop_front()
                                        .unwrap_or_else(|| guard.fallback.clone())
                                };

                                match reply {
                                    MockReply::DropConnection => {
                                        let _ = stream.shutdown().await;
                                        return;
                                    }
                                    MockReply::Json { status, body } => {
                                        let reason = match status {
                                            200 => "OK",
                                            500 => "Internal Server Error",
                                            503 => "Service Unavailable",
                                            _ => "Status",
                                        };
                                        let resp = format!(
                                            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
                                            status,
                                            reason,
                                            body.len(),
                                            body
                                        );
                                        if stream.write_all(resp.as_bytes()).await.is_err() {
                                            return;
                                        }
                                        if stream.flush().await.is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        });
                    }
                }
            }
        });

        Self {
            url,
            state,
            shutdown: Some(shutdown_tx),
        }
    }

    fn push_reply(&self, reply: MockReply) {
        self.state.lock().unwrap().replies.push_back(reply);
    }

    fn last_auth_header(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .auth_headers
            .last()
            .cloned()
            .flatten()
    }

    fn request_count(&self) -> usize {
        self.state.lock().unwrap().request_count
    }

    fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for MockHttpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[tokio::test]
async fn test_http_source_current_thread_runtime_reloads_fallback_and_u64() {
    let large_u64: u64 = (i64::MAX as u64) + 999_999;
    let v1 = format!(
        r#"{{"service_name":"alpha","port":8080,"max_quota_bytes":{}}}"#,
        large_u64
    );
    let v2 = format!(
        r#"{{"service_name":"alpha-v2","port":8081,"max_quota_bytes":{}}}"#,
        u64::MAX
    );
    let v3 = r#"{"service_name":"alpha-v3","port":8082,"max_quota_bytes":42}"#.to_string();

    let mut server = MockHttpServer::start(
        vec![
            MockReply::Json {
                status: 200,
                body: v1,
            },
            MockReply::Json {
                status: 200,
                body: v2,
            },
            MockReply::Json {
                status: 200,
                body: v3,
            },
        ],
        MockReply::Json {
            status: 500,
            body: r#"{"error":"boom"}"#.to_string(),
        },
    )
    .await;

    let http_source = HttpSource::builder()
        .with_url(&server.url)
        .with_auth_token("super-secret-bearer")
        .with_timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    // Debug output must redact bearer token
    let debug_str = format!("{:?}", http_source);
    assert!(!debug_str.contains("super-secret-bearer"));
    assert!(debug_str.contains("<redacted>"));

    // 1. Initial load inside a CurrentThread runtime (previously panicked via block_in_place!)
    let config = HotswapConfig::builder()
        .with_source(http_source)
        .build::<RemoteAppConfig>()
        .await
        .unwrap();

    assert_eq!(
        server.last_auth_header().as_deref(),
        Some("Bearer super-secret-bearer")
    );
    assert_eq!(config.get().service_name, "alpha");
    assert_eq!(config.get().port, 8080);
    assert_eq!(config.get().max_quota_bytes, large_u64);

    // 2. Two consecutive reloads across temporary runtimes / keep-alive connections
    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "alpha-v2");
    assert_eq!(config.get().port, 8081);
    assert_eq!(config.get().max_quota_bytes, u64::MAX);

    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "alpha-v3");
    assert_eq!(config.get().port, 8082);
    assert_eq!(config.get().max_quota_bytes, 42);
    assert_eq!(server.request_count(), 3);

    // 3. HTTP 500 falls back to cached last_known_good
    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "alpha-v3");
    assert_eq!(config.get().port, 8082);

    // 4. Abrupt connection drop falls back to cached last_known_good
    server.push_reply(MockReply::DropConnection);
    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "alpha-v3");

    // 5. Malformed JSON on HTTP 200 MUST return an error and NOT overwrite active config
    server.push_reply(MockReply::Json {
        status: 200,
        body: "{malformed-json".to_string(),
    });
    let err = config.reload().await.unwrap_err();
    assert!(matches!(err, ConfigError::DeserializationError(_)));
    assert_eq!(config.get().service_name, "alpha-v3");

    // 6. Server completely stopped (connection refused) still falls back to last_known_good
    server.stop();
    tokio::time::sleep(Duration::from_millis(20)).await;
    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "alpha-v3");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_http_source_multi_thread_runtime_basic_auth_and_reloads() {
    let server = MockHttpServer::start(
        vec![
            MockReply::Json {
                status: 200,
                body: r#"{"service_name":"mt-1","port":9000,"max_quota_bytes":100}"#.to_string(),
            },
            MockReply::Json {
                status: 200,
                body: r#"{"service_name":"mt-2","port":9001,"max_quota_bytes":200}"#.to_string(),
            },
            MockReply::Json {
                status: 503,
                body: r#"{"error":"unavailable"}"#.to_string(),
            },
            MockReply::DropConnection,
            MockReply::Json {
                status: 200,
                body: r#"{"service_name":"mt-3","port":9002,"max_quota_bytes":300}"#.to_string(),
            },
        ],
        MockReply::Json {
            status: 200,
            body: r#"{"service_name":"mt-3","port":9002,"max_quota_bytes":300}"#.to_string(),
        },
    )
    .await;

    let http_source = HttpSource::builder()
        .with_url(&server.url)
        .with_basic_auth("admin", "s3cr3t")
        .with_timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    let debug_str = format!("{:?}", http_source);
    assert!(debug_str.contains("admin"));
    assert!(!debug_str.contains("s3cr3t"));

    let config = HotswapConfig::builder()
        .with_source(http_source)
        .build::<RemoteAppConfig>()
        .await
        .unwrap();

    // Base64("admin:s3cr3t") == "YWRtaW46czNjcjN0"
    assert_eq!(
        server.last_auth_header().as_deref(),
        Some("Basic YWRtaW46czNjcjN0")
    );
    assert_eq!(config.get().service_name, "mt-1");

    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "mt-2");
    assert_eq!(config.get().port, 9001);

    // 503 -> fallback keeps mt-2
    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "mt-2");

    // Connection drop -> fallback keeps mt-2
    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "mt-2");

    // Recovery -> mt-3
    config.reload().await.unwrap();
    assert_eq!(config.get().service_name, "mt-3");
    assert_eq!(config.get().port, 9002);
}

#[tokio::test]
async fn test_http_source_initial_error_and_disabled_fallback() {
    let server = MockHttpServer::start(
        vec![
            // 1st request: initial load fails (no cached config yet)
            MockReply::Json {
                status: 500,
                body: r#"{"error":"down"}"#.to_string(),
            },
            // 2nd request: initial load succeeds with fallback disabled
            MockReply::Json {
                status: 200,
                body: r#"{"service_name":"strict","port":7070,"max_quota_bytes":10}"#.to_string(),
            },
            // 3rd request: reload fails because fallback_to_cached is false
            MockReply::Json {
                status: 500,
                body: r#"{"error":"down again"}"#.to_string(),
            },
        ],
        MockReply::Json {
            status: 500,
            body: r#"{"error":"down"}"#.to_string(),
        },
    )
    .await;

    // Even with fallback enabled, initial load with empty cache must fail on HTTP 500
    let initial_fail_source = HttpSource::builder()
        .with_url(&server.url)
        .with_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let initial_res = HotswapConfig::builder()
        .with_source(initial_fail_source)
        .build::<RemoteAppConfig>()
        .await;
    assert!(matches!(initial_res, Err(ConfigError::LoadError(_))));

    // With `with_fallback_to_cached(false)`, subsequent HTTP 500 returns `Err`
    let strict_source = HttpSource::builder()
        .with_url(&server.url)
        .with_timeout(Duration::from_secs(2))
        .with_fallback_to_cached(false)
        .build()
        .unwrap();
    let config = HotswapConfig::builder()
        .with_source(strict_source)
        .build::<RemoteAppConfig>()
        .await
        .unwrap();
    assert_eq!(config.get().service_name, "strict");

    let reload_err = config.reload().await.unwrap_err();
    assert!(matches!(reload_err, ConfigError::LoadError(_)));
    // Active config remains valid even when reload returns Err
    assert_eq!(config.get().service_name, "strict");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_http_source_load_from_plain_os_thread() {
    let server = MockHttpServer::start(
        vec![MockReply::Json {
            status: 200,
            body: r#"{"service_name":"os-thread","port":6060,"max_quota_bytes":77}"#.to_string(),
        }],
        MockReply::Json {
            status: 200,
            body: r#"{"service_name":"os-thread","port":6060,"max_quota_bytes":77}"#.to_string(),
        },
    )
    .await;

    let url = server.url.clone();
    let map = tokio::task::spawn_blocking(move || {
        // Spawn a fresh non-Tokio OS thread to ensure zero ambient runtime context
        std::thread::spawn(move || {
            let source = HttpSource::builder()
                .with_url(url)
                .with_timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            source.load().unwrap()
        })
        .join()
        .unwrap()
    })
    .await
    .unwrap();

    assert_eq!(
        map.get("service_name")
            .unwrap()
            .clone()
            .into_string()
            .unwrap(),
        "os-thread"
    );
}
