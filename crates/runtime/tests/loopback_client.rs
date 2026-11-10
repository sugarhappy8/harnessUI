//! The HTTP client for llama-server ignores proxy settings and never follows redirects
//! (ARCHITECTURE.md 7.3 and 13.6): a system proxy would otherwise receive local chat and the
//! per-launch key.
//!
//! This binary holds a single test because it sets proxy variables for the whole process.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use runtime::llama::server::loopback_client_builder;

/// Answers every request with a redirect to another loopback path, and counts requests.
fn redirecting_server() -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            // Read the request head.
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                line.clear();
            }
            let reply = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (port, hits)
}

#[test]
fn loopback_client_ignores_proxies_and_does_not_follow_redirects() {
    // A "proxy" on a port nothing listens on: a client that used it would fail to connect.
    let dead = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let proxy = format!(
        "http://127.0.0.1:{}",
        dead.local_addr().expect("addr").port()
    );
    drop(dead);
    // SAFETY: this is the only test in this binary and it sets the variables before it
    // starts any thread or runtime, so nothing reads the environment concurrently.
    unsafe {
        for name in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            std::env::set_var(name, &proxy);
        }
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
    }

    let (port, hits) = redirecting_server();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        // Control: an ordinary client honours the variables and fails at the dead proxy, so
        // this test would catch a loopback client that used them. (A machine whose OS proxy
        // settings exempt loopback makes the control inconclusive, not the test wrong.)
        let ordinary = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("client");
        if ordinary
            .get(format!("http://127.0.0.1:{port}/props"))
            .send()
            .await
            .is_ok()
        {
            eprintln!("note: the proxy variables did not affect an ordinary client here");
        }
        let before = hits.load(std::sync::atomic::Ordering::SeqCst);

        let client = loopback_client_builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("client");
        let response = client
            .get(format!("http://127.0.0.1:{port}/props"))
            .send()
            .await
            .expect("reached the loopback server directly, not through the proxy");
        assert_eq!(response.status(), 302, "the redirect was not followed");
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst) - before,
            1,
            "exactly one request reached the server"
        );
    });
}
