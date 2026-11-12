//! The real reader on the system the tests run on (CI runs `cargo test --workspace` on
//! Linux, Windows and an Apple silicon Mac). Graphics may be any reading: runners have no
//! graphics card, or a virtual one, or counters a sandbox hides.

use std::time::{Duration, Instant};
use usage::{Focus, Gap, Reading, system_sampler};

/// The app's reader treats a sample that takes longer than this as stuck (item 6, section 5).
const STUCK_AFTER: Duration = Duration::from_secs(2);

#[test]
fn reports_plausible_values_here() {
    let started = Instant::now();
    let mut sampler = system_sampler();
    let made_in = started.elapsed();
    let focus = Focus::default();

    let started = Instant::now();
    let first = sampler.sample(&focus);
    let first_in = started.elapsed();
    assert_eq!(
        first.processor_percent,
        Reading::Measuring,
        "a rate needs two samples"
    );
    assert!(first.threads > 0);
    assert_eq!(first.threads, usage::logical_processors());

    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    let second = sampler.sample(&focus);
    let second_in = started.elapsed();
    // Shown in the CI log of each system (with --nocapture, or when the test fails).
    eprintln!(
        "made in {made_in:?}, first sample in {first_in:?}, second in {second_in:?}: {}",
        serde_json::to_string(&second).unwrap()
    );
    assert!(first_in < STUCK_AFTER, "the first sample took {first_in:?}");
    assert!(
        second_in < STUCK_AFTER,
        "the second sample took {second_in:?}"
    );
    match &second.processor_percent {
        Reading::Value { value } => assert!(
            (0.0..=100.0).contains(value),
            "processor {value} % is not a percentage"
        ),
        other => panic!("the second sample has no processor figure: {other:?}"),
    }
    assert!(second.threads > 0);

    if let Reading::Value { value: g } = &second.graphics {
        assert!(g.name.chars().count() <= usage::NAME_CAP);
        if let Reading::Value { value } = g.busy_percent {
            assert!((0.0..=100.0).contains(&value));
        }
        if let Reading::Value { value } = g.memory_used_bytes {
            assert!(value <= usage::MAX_BYTES);
        }
    }

    #[cfg(target_os = "linux")]
    {
        let any_card = std::fs::read_dir("/sys/class/drm")
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name.strip_prefix("card")
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                })
            })
            .unwrap_or(false);
        if !any_card {
            assert_eq!(second.graphics, Reading::gap(Gap::NoGraphicsCard));
        }
        assert!(!second.unified);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = Gap::NoGraphicsCard;

    #[cfg(windows)]
    assert!(!second.unified);
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert!(second.unified);
}

#[test]
fn a_fresh_sampler_starts_at_measuring() {
    // A fresh sampler is in the same state as one after a gap of more than 10 s; the gap
    // rule itself is tested on the rate (`rates_start_again_after_a_gap` in lib.rs).
    let mut sampler = system_sampler();
    let focus = Focus {
        description: Some("A card that is not here".into()),
        total_hint_bytes: Some(8 << 30),
    };
    assert_eq!(sampler.sample(&focus).processor_percent, Reading::Measuring);
}

#[test]
fn the_sampler_moves_to_another_thread() {
    // The app's reader thread owns the sampler; it is made on one thread and used on another.
    let sampler = system_sampler();
    let handle = std::thread::spawn(move || {
        let mut sampler = sampler;
        sampler.sample(&Focus::default()).threads
    });
    assert!(handle.join().unwrap() > 0);
}
