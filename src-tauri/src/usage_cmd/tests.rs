//! The usage reader with a fake sampler (0.0.6 item 6, WP10d): the thread starts on
//! request and ends after its idle time, a stuck sample answers "readFailed" at once and
//! starts one replacement per run, computer memory comes from the system.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::*;

/// A sampler that counts the live samplers (made on the reader thread, dropped when it
/// ends) and takes `delay` per sample.
struct Fake {
    live: Arc<AtomicUsize>,
    delay: Duration,
    samples: u32,
}

impl Sampler for Fake {
    fn sample(&mut self, focus: &Focus) -> Sample {
        std::thread::sleep(self.delay);
        self.samples += 1;
        Sample {
            graphics: match &focus.description {
                Some(name) => Reading::value(usage::GraphicsSample {
                    name: name.clone(),
                    busy_percent: Reading::value(63.0),
                    memory_used_bytes: Reading::value(12 * MIB),
                    memory_total_bytes: focus.total_hint_bytes,
                }),
                None => Reading::gap(Gap::NoGraphicsCard),
            },
            processor_percent: if self.samples == 1 {
                Reading::Measuring
            } else {
                Reading::value(12.0)
            },
            threads: 16,
            unified: false,
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

fn reader(
    live: &Arc<AtomicUsize>,
    delay: Duration,
    idle: Duration,
    within: Duration,
) -> UsageReader {
    let live = Arc::clone(live);
    UsageReader::with_factory(
        Arc::new(move || {
            live.fetch_add(1, Ordering::SeqCst);
            Box::new(Fake {
                live: Arc::clone(&live),
                delay,
                samples: 0,
            })
        }),
        idle,
        within,
    )
}

async fn wait_until(what: &str, check: impl Fn() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what}");
}

#[test]
fn the_limits_are_thirty_and_two_seconds() {
    assert_eq!(IDLE_END, Duration::from_secs(30));
    assert_eq!(ANSWER_WITHIN, Duration::from_secs(2));
}

#[tokio::test]
async fn the_thread_starts_on_request_and_ends_when_idle() {
    let live = Arc::new(AtomicUsize::new(0));
    let r = reader(
        &live,
        Duration::ZERO,
        Duration::from_millis(200),
        Duration::from_secs(2),
    );
    assert_eq!(
        (r.threads_started(), live.load(Ordering::SeqCst)),
        (0, 0),
        "nothing before a request"
    );
    let first = r.sample(Focus::default()).await.expect("a sample");
    assert_eq!(
        first.processor_percent,
        Reading::Measuring,
        "the first sample"
    );
    let second = r.sample(Focus::default()).await.unwrap();
    assert_eq!(second.processor_percent, Reading::value(12.0));
    assert_eq!(r.threads_started(), 1, "one thread for both");
    assert_eq!(live.load(Ordering::SeqCst), 1);
    // No request for the idle time: the thread ends and drops its sampler.
    wait_until("the reader to end", || live.load(Ordering::SeqCst) == 0).await;
    // The next request starts another, which measures from the start again.
    let again = r.sample(Focus::default()).await.unwrap();
    assert_eq!(again.processor_percent, Reading::Measuring);
    assert_eq!(r.threads_started(), 2);
    // Requests within the idle time keep it alive.
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(r.sample(Focus::default()).await.is_some());
    }
    assert_eq!(r.threads_started(), 2);
}

#[tokio::test]
async fn a_stuck_sample_answers_at_once_and_starts_one_replacement() {
    let live = Arc::new(AtomicUsize::new(0));
    // Every sample takes 400 ms; the answer is due within 100 ms.
    let r = reader(
        &live,
        Duration::from_millis(400),
        Duration::from_secs(30),
        Duration::from_millis(100),
    );
    let started = Instant::now();
    assert_eq!(r.sample(Focus::default()).await, None, "past the limit");
    assert!(started.elapsed() < Duration::from_millis(300));
    // The reader is stuck: the next request answers at once and starts the replacement.
    let started = Instant::now();
    assert_eq!(r.sample(Focus::default()).await, None);
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(r.threads_started(), 2);
    // The replacement is slow as well: stuck again, and no third thread this run.
    assert_eq!(r.sample(Focus::default()).await, None);
    for _ in 0..3 {
        let started = Instant::now();
        assert_eq!(r.sample(Focus::default()).await, None);
        assert!(started.elapsed() < Duration::from_millis(50));
    }
    assert_eq!(r.threads_started(), 2, "one replacement per run");
}

#[tokio::test]
async fn a_slow_but_finished_sample_unsticks_the_reader() {
    let live = Arc::new(AtomicUsize::new(0));
    let slow_once = Arc::new(AtomicUsize::new(0));
    let factory_live = Arc::clone(&live);
    let first_slow = Arc::clone(&slow_once);
    struct SlowOnce {
        inner: Fake,
        slow: Arc<AtomicUsize>,
    }
    impl Sampler for SlowOnce {
        fn sample(&mut self, focus: &Focus) -> Sample {
            if self.slow.fetch_add(1, Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(250));
            }
            self.inner.sample(focus)
        }
    }
    let r = UsageReader::with_factory(
        Arc::new(move || {
            factory_live.fetch_add(1, Ordering::SeqCst);
            Box::new(SlowOnce {
                inner: Fake {
                    live: Arc::clone(&factory_live),
                    delay: Duration::ZERO,
                    samples: 0,
                },
                slow: Arc::clone(&first_slow),
            })
        }),
        Duration::from_secs(30),
        Duration::from_millis(100),
    );
    assert_eq!(r.sample(Focus::default()).await, None);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        r.sample(Focus::default()).await.is_some(),
        "its call returned"
    );
    assert_eq!(r.threads_started(), 1, "no replacement needed");
}

#[test]
fn computer_memory_comes_from_the_system() {
    let sample = Sample {
        graphics: Reading::gap(Gap::NoGraphicsCard),
        processor_percent: Reading::value(12.0),
        threads: 16,
        unified: false,
    };
    let memory = models::hardware::SystemMemory {
        total_bytes: 32 * 1024 * MIB,
        available_bytes: 15 * 1024 * MIB,
    };
    let now = answer(Some(sample.clone()), Some(memory));
    assert_eq!(
        now.memory,
        Reading::value(MemUse {
            used_bytes: 17 * 1024 * MIB,
            total_bytes: 32 * 1024 * MIB,
        })
    );
    // The JSON the webview reads (src/lib/types.ts UsageNow): the sample's fields flat.
    let v = serde_json::to_value(&now).unwrap();
    assert_eq!(
        v["graphics"],
        json!({"kind": "notAvailable", "why": "noGraphicsCard"})
    );
    assert_eq!(
        v["processorPercent"],
        json!({"kind": "value", "value": 12.0})
    );
    assert_eq!(v["threads"], json!(16));
    assert_eq!(v["unified"], json!(false));
    assert_eq!(
        v["memory"],
        json!({"kind": "value", "value": {"usedBytes": 18_253_611_008u64, "totalBytes": 34_359_738_368u64}})
    );
    // The system says nothing: not available. More available than total: 0 in use.
    assert_eq!(
        answer(Some(sample.clone()), None).memory,
        Reading::gap(Gap::ReadFailed)
    );
    let odd = models::hardware::SystemMemory {
        total_bytes: 1,
        available_bytes: 5,
    };
    assert_eq!(
        answer(Some(sample), Some(odd)).memory,
        Reading::value(MemUse {
            used_bytes: 0,
            total_bytes: 1
        })
    );
    // A stuck reader: every figure "Couldn't read this just now".
    let stuck = answer(None, Some(memory));
    assert_eq!(stuck.sample.graphics, Reading::gap(Gap::ReadFailed));
    assert_eq!(
        stuck.sample.processor_percent,
        Reading::gap(Gap::ReadFailed)
    );
    assert_eq!(stuck.memory, Reading::gap(Gap::ReadFailed));
    assert!(stuck.sample.threads >= 1);
}

fn probe(
    kind: runtime::BackendKind,
    devices: &[(&str, &str, u64, u64)],
) -> (runtime::Backend, runtime::ProbeResult) {
    (
        runtime::Backend {
            kind,
            dir: std::path::PathBuf::from("/b"),
            server: std::path::PathBuf::from("/b/llama-server"),
            fit_params: None,
            build: "b1".into(),
            source: runtime::BackendSource::Installed,
        },
        runtime::ProbeResult {
            backend_kind: kind,
            devices: devices
                .iter()
                .map(|(name, description, total, free)| runtime::Device {
                    name: (*name).into(),
                    description: (*description).into(),
                    total_mib: *total,
                    free_mib: *free,
                })
                .collect(),
            error: None,
            output: Vec::new(),
        },
    )
}

#[test]
fn the_focus_is_the_loaded_models_device_else_the_one_a_load_would_use() {
    let found = vec![
        probe(
            runtime::BackendKind::Vulkan,
            &[
                ("Vulkan0", "AMD Radeon(TM) Graphics", 512, 400),
                ("Vulkan1", "AMD Radeon RX 9070 XT", 16_304, 15_000),
            ],
        ),
        probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)]),
    ];
    let defaults = LocalModelSettings::default();
    // Nothing loaded: the card a load would pick (the one with the most memory).
    let f = focus(&found, &RuntimeStatusDto::default(), &defaults);
    assert_eq!(f.description.as_deref(), Some("AMD Radeon RX 9070 XT"));
    assert_eq!(f.total_hint_bytes, Some(16_304 * MIB));
    // The defaults name the other card.
    let named = LocalModelSettings {
        device: Some("Vulkan0".into()),
        ..LocalModelSettings::default()
    };
    let f = focus(&found, &RuntimeStatusDto::default(), &named);
    assert_eq!(f.description.as_deref(), Some("AMD Radeon(TM) Graphics"));
    // The loaded model's device wins.
    let loaded = RuntimeStatusDto {
        state: RuntimeStateDto::Ready,
        backend: Some("vulkan".into()),
        device: Some("Vulkan0".into()),
        ..RuntimeStatusDto::default()
    };
    let f = focus(&found, &loaded, &defaults);
    assert_eq!(f.description.as_deref(), Some("AMD Radeon(TM) Graphics"));
    // On the processor, or with nothing probed yet: no device.
    let cpu = RuntimeStatusDto {
        state: RuntimeStateDto::Ready,
        backend: Some("cpu".into()),
        ..RuntimeStatusDto::default()
    };
    let only_cpu = vec![probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)])];
    assert_eq!(focus(&only_cpu, &cpu, &defaults), Focus::default());
    assert_eq!(
        focus(&[], &RuntimeStatusDto::default(), &defaults),
        Focus::default()
    );
}

/// Both item 6 commands are granted to the main window only and check the calling
/// window's label first.
#[test]
fn the_commands_are_for_the_main_window_only() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let read = |p: std::path::PathBuf| std::fs::read_to_string(&p).unwrap();
    let caps = manifest.join("capabilities");
    let main = read(caps.join("main.json"));
    let list = read(manifest.join("commands.list"));
    let lib = read(manifest.join("src").join("lib.rs"));
    for (command, file) in [
        ("offload_measure", "models_cmd.rs"),
        ("usage_now", "usage_cmd.rs"),
    ] {
        let grant = format!("\"allow-{}\"", command.replace('_', "-"));
        assert!(main.contains(&grant), "main.json lacks {grant}");
        assert!(list.lines().any(|l| l.trim() == command), "{command}");
        assert!(
            lib.contains(&format!("::{command},")),
            "{command} is not registered"
        );
        for other in ["overlay.json", "indicator.json"] {
            assert!(!read(caps.join(other)).contains(&grant), "{other}: {grant}");
        }
        let source = read(manifest.join("src").join(file));
        let start = source
            .find(&format!("pub async fn {command}("))
            .unwrap_or_else(|| panic!("{command} in {file}"));
        let body = &source[start..];
        let open = body.find('{').unwrap();
        let first = body[open + 1..].trim_start();
        assert!(
            first.starts_with("crate::commands::only_main_window(window.label())?;"),
            "{command} must check the window first"
        );
    }
}
