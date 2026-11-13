//! Fine control (0.0.6 item 6, WP10d): storage, checks, the resolver, Automatic and the
//! marks of the settings panel. The numbers are fixed (no file, no probe), except the
//! split-file and real-model tests at the end.

use models::Verdict;
use models::layers::{LayerSize, LayerSizes};
use models::placement::Fits;
use serde::Deserialize;
use serde_json::json;

use super::*;
use crate::offload::{EstimateWhy, PictureSource, Placer};

const GB: u64 = 1_000_000_000;

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

/// The owner's RX 9070 XT (16 GB) with `free_mib` free, 32 GB of computer memory free of
/// 64, 16 threads.
fn card(free_mib: u64) -> Machine {
    let mut m = Machine::from_probes(
        &[
            probe(
                runtime::BackendKind::Vulkan,
                &[("Vulkan0", "AMD Radeon RX 9070 XT", 16_304, free_mib)],
            ),
            probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)]),
        ],
        None,
        None,
        32 * GB,
        &Freed::default(),
    );
    m.threads = 16;
    m.ram_total_bytes = 64 * GB;
    m
}

/// No graphics device: the processor only.
fn processor_only() -> Machine {
    let mut m = Machine::from_probes(
        &[probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)])],
        None,
        None,
        32 * GB,
        &Freed::default(),
    );
    m.threads = 16;
    m.ram_total_bytes = 64 * GB;
    m
}

/// A 16 GB Mac: Apple graphics may use 10,922 MiB.
fn mac() -> Machine {
    let mut m = Machine::from_probes(
        &[
            probe(
                runtime::BackendKind::Metal,
                &[("MTL0", "Apple M2", 10_922, 10_922)],
            ),
            probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)]),
        ],
        Some("metal"),
        None,
        12 * GB,
        &Freed::default(),
    );
    m.threads = 8;
    m.ram_total_bytes = 16 * 1024 * 1024 * 1024;
    m
}

/// A dense model: 40 blocks of 100 MB, input and output 200 MB each (41 units, 4.4 GB),
/// 2,048 bytes of conversation memory per block and token (f16), trained for 40,960.
fn dense() -> ModelFacts {
    model(40, 100_000_000, 0, 200_000_000)
}

/// A mixture of experts: 48 blocks of 20 MB dense and 200 MB expert parts.
fn moe() -> ModelFacts {
    model(48, 20_000_000, 200_000_000, 100_000_000)
}

fn model(blocks: u32, dense: u64, expert: u64, edge: u64) -> ModelFacts {
    let sizes = LayerSizes {
        input_bytes: edge,
        output_bytes: edge,
        layers: vec![
            LayerSize {
                dense_bytes: dense,
                expert_bytes: expert,
            };
            blocks as usize
        ],
        other_bytes: 0,
    };
    let total = sizes.total();
    ModelFacts {
        file_name: "model-Q4_K_M.gguf".into(),
        size_bytes: total,
        weights_bytes: total,
        projector_bytes: 0,
        header: Some(HeaderFacts {
            trained_ctx: Some(40_960),
            kv_f16: u64::from(blocks) * 2048,
            kv_q8: u64::from(blocks) * 1088,
            vocab: 32_000,
            thinking: false,
        }),
        layers: Some(LayerFacts {
            blocks,
            experts: expert > 0,
            sizes: Some(sizes),
            kv_f16: vec![2048; blocks as usize],
            kv_q8: vec![1088; blocks as usize],
        }),
    }
}

fn auto_defaults() -> LocalModelSettings {
    LocalModelSettings {
        context_auto: true,
        ..LocalModelSettings::default()
    }
}

fn fine(f: FineControl) -> ModelLoadSettings {
    ModelLoadSettings {
        fine: f,
        ..ModelLoadSettings::default()
    }
}

/// The placement words a load of `r` puts after `--device` (up to `--cache-ram`), with
/// `device` as the launch's device (`None`: the processor; the app then launches with
/// Don't use it, as `runtime_manager::launch_config` does).
fn placement_words(r: &ResolvedLoad, device: Option<&str>) -> String {
    let launch = runtime::LaunchConfig {
        device: device.map(str::to_owned),
        gpu: if device.is_some() { r.gpu } else { GpuUse::Off },
        save_memory: r.save_memory,
        fine: r.fine.clone(),
        ..runtime::LaunchConfig::new("/m.gguf".into(), r.ctx, 1, "k".into())
    };
    let args = runtime::build_args(&launch);
    let at = |w: &str| args.iter().position(|a| a == w).unwrap();
    args[at("--device")..at("--cache-ram")].join(" ")
}

#[test]
fn fine_control_old_json_both_ways() {
    // 0.0.6 JSON written before item 6 reads with every Fine control value Automatic, and
    // a model without Fine control is written exactly as before (no "fine" key).
    let before = json!({
        "context": {"fixed": 8192},
        "gpu": "fit",
        "saveMemory": "on",
        "keepInMemory": "default",
        "device": "auto",
        "thinkingLimit": "short"
    });
    let read: ModelLoadSettings = serde_json::from_value(before.clone()).unwrap();
    assert!(read.fine.is_default());
    assert_eq!(read.gpu, GpuChoice::Fit);
    assert_eq!(serde_json::to_value(&read).unwrap(), before);
    // Fine control's shapes (src/lib/types.ts).
    let all = FineControl {
        layers: LayersChoice::Count(30),
        experts: ExpertsChoice::FirstLayers(20),
        kv: KvPlace::ComputerMemory,
        threads: ThreadsChoice::Count(8),
        fast_attention: FastAttention::Off,
        leave_free: LeaveFree::Mib(2048),
    };
    let with = ModelLoadSettings {
        gpu: GpuChoice::Fit,
        save_memory: Toggle::On,
        ..fine(all)
    };
    let v = serde_json::to_value(&with).unwrap();
    assert_eq!(
        v["fine"],
        json!({
            "layers": {"count": 30},
            "experts": {"firstLayers": 20},
            "kv": "computerMemory",
            "threads": {"count": 8},
            "fastAttention": "off",
            "leaveFree": {"mib": 2048}
        })
    );
    assert_eq!(
        serde_json::from_value::<ModelLoadSettings>(v.clone()).unwrap(),
        with
    );
    assert_eq!(
        serde_json::to_value(FineControl {
            experts: ExpertsChoice::All,
            ..FineControl::default()
        })
        .unwrap()["experts"],
        json!("all")
    );
    assert_eq!(
        serde_json::to_value(FineControl::default()).unwrap(),
        json!({"layers": "auto", "experts": "auto", "kv": "auto", "threads": "auto",
               "fastAttention": "auto", "leaveFree": "auto"})
    );
    // A partial object fills the rest with Automatic.
    let partial: ModelLoadSettings =
        serde_json::from_value(json!({"fine": {"threads": {"count": 4}}})).unwrap();
    assert_eq!(
        partial.fine,
        FineControl {
            threads: ThreadsChoice::Count(4),
            ..FineControl::default()
        }
    );
    // The struct of 0.0.6 without item 6 (copied here) reads JSON with "fine" and keeps the
    // other fields.
    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct WithoutItem6 {
        #[serde(default)]
        context: ContextChoice,
        #[serde(default)]
        gpu: GpuChoice,
        #[serde(default)]
        save_memory: Toggle,
        #[serde(default)]
        keep_in_memory: Toggle,
        #[serde(default)]
        device: DeviceChoice,
        #[serde(default)]
        thinking_limit: ThinkingChoice,
    }
    let old: WithoutItem6 = serde_json::from_value(v).unwrap();
    assert_eq!((old.gpu, old.save_memory), (GpuChoice::Fit, Toggle::On));
    // Values outside the lists are refused when read.
    for bad in [
        json!({"fine": {"layers": "all"}}),
        json!({"fine": {"layers": {"count": -1}}}),
        json!({"fine": {"kv": "disk"}}),
        json!({"fine": {"threads": {"count": 70000}}}),
        json!({"fine": {"leaveFree": "lots"}}),
        json!({"fine": {"fastAttention": true}}),
    ] {
        assert!(
            serde_json::from_value::<ModelLoadSettings>(bad.clone()).is_err(),
            "{bad}"
        );
    }
    // A resolved load written before item 6 reads without Fine control.
    let r: ResolvedLoad = serde_json::from_value(json!({
        "ctx": 4096, "ctxChoice": {"fixed": 4096}, "ctxSource": "modelSetting", "gpu": "all",
        "saveMemory": false, "keepInMemory": false, "thinkingLimit": "noLimit",
        "backend": null, "device": null, "mmproj": null
    }))
    .unwrap();
    assert_eq!(r.fine, FineLaunch::default());
}

#[test]
fn fine_values_are_checked_against_their_lists_and_the_model() {
    let refused = Err(FINE_REFUSED);
    assert_eq!(FINE_REFUSED, "This setting isn't one of the choices.");
    let d = dense();
    let m = moe();
    let check = |f: FineControl, facts: Option<&ModelFacts>| validate_fine(&f, facts);
    let layers = |n| FineControl {
        layers: LayersChoice::Count(n),
        ..FineControl::default()
    };
    let threads = |n| FineControl {
        threads: ThreadsChoice::Count(n),
        ..FineControl::default()
    };
    let experts = |e| FineControl {
        experts: e,
        ..FineControl::default()
    };
    let leave = |mib| FineControl {
        leave_free: LeaveFree::Mib(mib),
        ..FineControl::default()
    };
    assert_eq!(check(FineControl::default(), Some(&d)), Ok(()));
    // Layers: 0 to the model's units (41), at most 1,000 without facts.
    assert_eq!(check(layers(0), Some(&d)), Ok(()));
    assert_eq!(check(layers(41), Some(&d)), Ok(()));
    assert_eq!(check(layers(42), Some(&d)), refused);
    assert_eq!(check(layers(1000), None), Ok(()));
    assert_eq!(check(layers(1001), None), refused);
    // Threads: 1 to 1,024.
    assert_eq!(check(threads(1), Some(&d)), Ok(()));
    assert_eq!(check(threads(1024), Some(&d)), Ok(()));
    assert_eq!(check(threads(0), Some(&d)), refused);
    assert_eq!(check(threads(1025), None), refused);
    // Leave free: 512, 2,048 or 4,096 MiB.
    for mib in LEAVE_FREE_MIB {
        assert_eq!(check(leave(mib), Some(&d)), Ok(()), "{mib}");
    }
    for mib in [0, 1024, 1000, 8192] {
        assert_eq!(check(leave(mib), None), refused, "{mib}");
    }
    // Expert parts: only for a model that has them, 1 to its expert layers (48).
    assert_eq!(check(experts(ExpertsChoice::All), Some(&d)), refused);
    assert_eq!(
        check(experts(ExpertsChoice::FirstLayers(1)), Some(&d)),
        refused
    );
    assert_eq!(check(experts(ExpertsChoice::All), Some(&m)), Ok(()));
    assert_eq!(
        check(experts(ExpertsChoice::FirstLayers(1)), Some(&m)),
        Ok(())
    );
    assert_eq!(
        check(experts(ExpertsChoice::FirstLayers(48)), Some(&m)),
        Ok(())
    );
    assert_eq!(
        check(experts(ExpertsChoice::FirstLayers(49)), Some(&m)),
        refused
    );
    assert_eq!(
        check(experts(ExpertsChoice::FirstLayers(0)), Some(&m)),
        refused
    );
    assert_eq!(
        check(experts(ExpertsChoice::FirstLayers(1000)), None),
        Ok(())
    );
    assert_eq!(
        check(experts(ExpertsChoice::FirstLayers(1001)), None),
        refused
    );
    // A header that cannot be read: the ranges only.
    let unreadable = ModelFacts {
        header: None,
        layers: None,
        ..dense()
    };
    assert_eq!(check(layers(500), Some(&unreadable)), Ok(()));
    assert_eq!(
        check(experts(ExpertsChoice::All), Some(&unreadable)),
        Ok(())
    );
    // Every command's check includes them.
    let machine = card(15_000);
    assert_eq!(validate(&fine(threads(0)), Some(40_960), &machine), refused);
    assert_eq!(validate_for(&fine(layers(42)), &d, &machine), refused);
    assert_eq!(validate_for(&fine(layers(41)), &d, &machine), Ok(()));
}

#[tokio::test]
async fn a_stored_fine_value_outside_the_lists_launches_as_automatic() {
    let store = store::Store::open_in_memory().unwrap();
    let key = settings_key("m.gguf", 16);
    store
        .set_setting(
            &key,
            &json!({"saveMemory": "on", "fine": {
                "layers": {"count": 5000}, "experts": {"firstLayers": 0},
                "threads": {"count": 0}, "leaveFree": {"mib": 1000},
                "kv": "computerMemory", "fastAttention": "off"
            }}),
        )
        .await
        .unwrap();
    let o = read_overrides(&store, "m.gguf", 16).await;
    assert_eq!(o.save_memory, Toggle::On);
    assert_eq!(
        o.fine,
        FineControl {
            kv: KvPlace::ComputerMemory,
            fast_attention: FastAttention::Off,
            ..FineControl::default()
        },
        "each value outside its range is Automatic, the rest kept"
    );
    // What such a row launches: no layer count, threads or margin.
    let r = resolve(
        &ModelLoadSettings {
            gpu: GpuChoice::Fit,
            ..o
        },
        &auto_defaults(),
        None,
        Some(&dense()),
        &card(15_000),
    );
    assert_eq!(
        r.fine,
        FineLaunch {
            kv_in_computer_memory: true,
            ..FineLaunch::default()
        }
    );
    assert_eq!(
        placement_words(&r, Some("Vulkan0")),
        "--device Vulkan0 --fit on -nkvo -fa on -ctk q8_0 -ctv q8_0"
    );
    // Values at the ends of their ranges are kept.
    store
        .set_setting(
            &key,
            &json!({"fine": {"layers": {"count": 1000}, "experts": {"firstLayers": 1000},
                             "threads": {"count": 1024}, "leaveFree": {"mib": 4096}}}),
        )
        .await
        .unwrap();
    assert_eq!(
        read_overrides(&store, "m.gguf", 16).await.fine,
        FineControl {
            layers: LayersChoice::Count(1000),
            experts: ExpertsChoice::FirstLayers(1000),
            threads: ThreadsChoice::Count(1024),
            leave_free: LeaveFree::Mib(4096),
            ..FineControl::default()
        }
    );
}

/// A stored Fine control value 0.0.6 cannot read at all (too large for its type, negative,
/// an unknown choice, `null`, a later version's shape) is Automatic on its own, like a
/// value outside its range (3.2), and never takes the model's other choices with it.
#[tokio::test]
async fn a_stored_fine_value_of_the_wrong_type_resets_only_that_value() {
    let store = store::Store::open_in_memory().unwrap();
    let key = settings_key("m.gguf", 16);
    store
        .set_setting(
            &key,
            &json!({"gpu": "off", "context": {"fixed": 16384}, "saveMemory": "on",
                    "fine": {"threads": {"count": 70000}, "fastAttention": "off",
                             "kv": "computerMemory"}}),
        )
        .await
        .unwrap();
    let o = read_overrides(&store, "m.gguf", 16).await;
    assert_eq!(
        (o.gpu, o.context, o.save_memory),
        (GpuChoice::Off, ContextChoice::Fixed(16384), Toggle::On),
        "the plain choices are kept"
    );
    assert_eq!(
        o.fine,
        FineControl {
            kv: KvPlace::ComputerMemory,
            fast_attention: FastAttention::Off,
            ..FineControl::default()
        },
        "only the unreadable thread count is Automatic"
    );
    // Every unreadable shape, field by field, next to a readable value that is kept.
    for (field, bad) in [
        ("layers", json!("all")),
        ("layers", json!({"count": -1})),
        ("layers", json!({"count": 5_000_000_000_u64})),
        ("layers", json!({"count": 2.5})),
        ("experts", json!({"firstLayers": -3})),
        ("experts", json!("some")),
        ("kv", json!("disk")),
        ("threads", json!({"count": 70000})),
        ("threads", json!(null)),
        ("fastAttention", json!(true)),
        ("leaveFree", json!("lots")),
        ("leaveFree", json!({"mib": -512})),
        ("leaveFree", json!({"gib": 2})),
    ] {
        let mut row = json!({"gpu": "off", "fine": {"layers": {"count": 7},
                             "experts": "all", "kv": "computerMemory",
                             "threads": {"count": 3}, "fastAttention": "on",
                             "leaveFree": {"mib": 2048}}});
        row["fine"][field] = bad.clone();
        store.set_setting(&key, &row).await.unwrap();
        let o = read_overrides(&store, "m.gguf", 16).await;
        assert_eq!(o.gpu, GpuChoice::Off, "{field}: {bad}");
        let mut want = FineControl {
            layers: LayersChoice::Count(7),
            experts: ExpertsChoice::All,
            kv: KvPlace::ComputerMemory,
            threads: ThreadsChoice::Count(3),
            fast_attention: FastAttention::On,
            leave_free: LeaveFree::Mib(2048),
        };
        match field {
            "layers" => want.layers = LayersChoice::Auto,
            "experts" => want.experts = ExpertsChoice::Auto,
            "kv" => want.kv = KvPlace::Auto,
            "threads" => want.threads = ThreadsChoice::Auto,
            "fastAttention" => want.fast_attention = FastAttention::Auto,
            _ => want.leave_free = LeaveFree::Auto,
        }
        assert_eq!(o.fine, want, "{field}: {bad}");
    }
    // A "fine" that is not an object at all is all Automatic; the rest is kept.
    for bad in [json!(null), json!(5), json!("auto"), json!([1, 2])] {
        store
            .set_setting(&key, &json!({"gpu": "off", "fine": bad}))
            .await
            .unwrap();
        let o = read_overrides(&store, "m.gguf", 16).await;
        assert_eq!(
            (o.gpu, o.fine),
            (GpuChoice::Off, FineControl::default()),
            "{bad}"
        );
    }
    // Every Fine control value the app saves reads back as it was (the field names of the
    // lenient reader follow the stored JSON).
    let full = ModelLoadSettings {
        gpu: GpuChoice::Fit,
        fine: FineControl {
            layers: LayersChoice::Count(12),
            experts: ExpertsChoice::FirstLayers(4),
            kv: KvPlace::ComputerMemory,
            threads: ThreadsChoice::Count(6),
            fast_attention: FastAttention::Off,
            leave_free: LeaveFree::Mib(512),
        },
        ..ModelLoadSettings::default()
    };
    save_overrides(&store, "m.gguf", 16, &full).await.unwrap();
    assert_eq!(read_overrides(&store, "m.gguf", 16).await, full);
    // A row that is not readable at all still follows the defaults for all models.
    store
        .set_setting(
            &key,
            &json!({"gpu": "sideways", "fine": {"threads": {"count": 3}}}),
        )
        .await
        .unwrap();
    assert_eq!(
        read_overrides(&store, "m.gguf", 16).await.gpu,
        GpuChoice::Default
    );
    store.set_setting(&key, &json!([1, 2])).await.unwrap();
    assert_eq!(
        read_overrides(&store, "m.gguf", 16).await,
        ModelLoadSettings::default()
    );
    // Drafts from the webview stay strict: the same shapes are refused, not repaired.
    assert!(
        serde_json::from_value::<ModelLoadSettings>(json!({"fine": {"threads": {"count": 70000}}}))
            .is_err()
    );
}

/// Section 3.1, row by row, from the stored choices to the launch's words.
#[test]
fn fine_control_resolves_per_row_of_the_table() {
    let d = dense();
    let m = moe();
    let on_card = card(15_000);
    let words = |o: &ModelLoadSettings, facts: &ModelFacts, machine: &Machine, device| {
        let r = resolve(o, &auto_defaults(), None, Some(facts), machine);
        placement_words(&r, device)
    };
    let all = |f: FineControl| ModelLoadSettings {
        gpu: GpuChoice::All,
        ..fine(f)
    };
    let fit = |f: FineControl| ModelLoadSettings {
        gpu: GpuChoice::Fit,
        ..fine(f)
    };
    let dev = Some("Vulkan0");
    let layers = |n| FineControl {
        layers: LayersChoice::Count(n),
        ..FineControl::default()
    };
    let leave = FineControl {
        leave_free: LeaveFree::Mib(2048),
        ..FineControl::default()
    };
    // No custom placement.
    assert_eq!(
        words(&all(FineControl::default()), &d, &on_card, dev),
        "--device Vulkan0 -ngl all --fit off"
    );
    assert_eq!(
        words(&fit(FineControl::default()), &d, &on_card, dev),
        "--device Vulkan0 --fit on"
    );
    // Leave free only with As much as fits.
    assert_eq!(
        words(&fit(leave), &d, &on_card, dev),
        "--device Vulkan0 --fit on -fitt 2048"
    );
    assert_eq!(
        words(&all(leave), &d, &on_card, dev),
        "--device Vulkan0 -ngl all --fit off"
    );
    // Layers N replace both plain choices, with --fit off and no margin.
    assert_eq!(
        words(&all(layers(30)), &d, &on_card, dev),
        "--device Vulkan0 -ngl 30 --fit off"
    );
    assert_eq!(
        words(
            &fit(FineControl {
                leave_free: LeaveFree::Mib(2048),
                ..layers(30)
            }),
            &d,
            &on_card,
            dev
        ),
        "--device Vulkan0 -ngl 30 --fit off"
    );
    assert_eq!(
        words(&all(layers(0)), &d, &on_card, dev),
        "--device Vulkan0 -ngl 0 --fit off"
    );
    // At or above the model's units: every unit. With All of it that is plain All of it's
    // own `-ngl all` (no layer count, so the same launch); with As much as fits it is a
    // custom placement of every unit (`-ngl` of the unit count reads as all of them), not
    // `--fit on`.
    let plain_all = resolve(
        &all(FineControl::default()),
        &auto_defaults(),
        None,
        Some(&d),
        &on_card,
    );
    for n in [41, 50, 1000] {
        let r = resolve(&all(layers(n)), &auto_defaults(), None, Some(&d), &on_card);
        assert_eq!(r.fine.gpu_layers, None, "{n}");
        assert!(r.same_launch(&plain_all), "{n}");
        assert_eq!(
            placement_words(&r, dev),
            "--device Vulkan0 -ngl all --fit off",
            "{n}"
        );
        let r = resolve(&fit(layers(n)), &auto_defaults(), None, Some(&d), &on_card);
        assert_eq!(r.fine.gpu_layers, Some(41), "{n}");
        assert_eq!(
            placement_words(&r, dev),
            "--device Vulkan0 -ngl 41 --fit off",
            "{n}"
        );
    }
    assert_eq!(
        words(&all(layers(40)), &d, &on_card, dev),
        "--device Vulkan0 -ngl 40 --fit off"
    );
    // With expert parts on the processor, every unit is `-ngl all` with either choice.
    let all_units_experts = FineControl {
        experts: ExpertsChoice::All,
        ..layers(49)
    };
    for o in [all(all_units_experts), fit(all_units_experts)] {
        let r = resolve(&o, &auto_defaults(), None, Some(&moe()), &on_card);
        assert_eq!(r.fine.gpu_layers, None);
        assert_eq!(
            placement_words(&r, dev),
            "--device Vulkan0 -ngl all --fit off --cpu-moe"
        );
    }
    // A model whose units are unknown keeps the count as chosen.
    let unreadable = ModelFacts {
        header: None,
        layers: None,
        ..dense()
    };
    let r = resolve(
        &all(layers(50)),
        &auto_defaults(),
        None,
        Some(&unreadable),
        &on_card,
    );
    assert_eq!(r.fine.gpu_layers, Some(50));
    // Expert parts only, and with layers.
    let experts = |e| FineControl {
        experts: e,
        ..FineControl::default()
    };
    assert_eq!(
        words(&fit(experts(ExpertsChoice::All)), &m, &on_card, dev),
        "--device Vulkan0 -ngl all --fit off --cpu-moe"
    );
    assert_eq!(
        words(
            &all(FineControl {
                experts: ExpertsChoice::FirstLayers(20),
                ..layers(30)
            }),
            &m,
            &on_card,
            dev
        ),
        "--device Vulkan0 -ngl 30 --fit off --n-cpu-moe 20"
    );
    // The first k at or above the expert layers: all of them.
    assert_eq!(
        words(
            &all(experts(ExpertsChoice::FirstLayers(48))),
            &m,
            &on_card,
            dev
        ),
        "--device Vulkan0 -ngl all --fit off --cpu-moe"
    );
    // A model without expert parts drops them.
    assert_eq!(
        words(&all(experts(ExpertsChoice::All)), &d, &on_card, dev),
        "--device Vulkan0 -ngl all --fit off"
    );
    // Conversation memory in computer memory.
    let kv = FineControl {
        kv: KvPlace::ComputerMemory,
        ..FineControl::default()
    };
    assert_eq!(
        words(&all(kv), &d, &on_card, dev),
        "--device Vulkan0 -ngl all --fit off -nkvo"
    );
    // Everything set.
    let every = FineControl {
        layers: LayersChoice::Count(30),
        experts: ExpertsChoice::FirstLayers(20),
        kv: KvPlace::ComputerMemory,
        threads: ThreadsChoice::Count(8),
        fast_attention: FastAttention::Off,
        leave_free: LeaveFree::Mib(4096),
    };
    // Don't use it, saved here or in the defaults for all models, wins: only threads and
    // fast attention reach the command line.
    let off = ModelLoadSettings {
        gpu: GpuChoice::Off,
        ..fine(every)
    };
    let r = resolve(&off, &auto_defaults(), None, Some(&m), &on_card);
    assert_eq!(
        r.fine,
        FineLaunch {
            threads: Some(8),
            flash_attn: runtime::FlashAttn::Off,
            ..FineLaunch::default()
        }
    );
    assert_eq!(
        placement_words(&r, dev),
        "--device none -ngl 0 --fit off -t 8 -fa off"
    );
    let off_defaults = LocalModelSettings {
        gpu: GpuUse::Off,
        ..auto_defaults()
    };
    let r2 = resolve(&fine(every), &off_defaults, None, Some(&m), &on_card);
    assert_eq!(r2.fine, r.fine, "Don't use it from the defaults wins too");
    // No graphics device: the same.
    let r3 = resolve(
        &fine(every),
        &auto_defaults(),
        None,
        Some(&m),
        &processor_only(),
    );
    assert_eq!(r3.fine, r.fine);
    assert_eq!(
        placement_words(&r3, None),
        "--device none -ngl 0 --fit off -t 8 -fa off"
    );
    // Threads above this computer's become its threads; an unknown count keeps them.
    let many = fine(FineControl {
        threads: ThreadsChoice::Count(64),
        ..FineControl::default()
    });
    assert_eq!(
        resolve(&many, &auto_defaults(), None, Some(&d), &on_card)
            .fine
            .threads,
        Some(16)
    );
    let unknown = Machine {
        threads: 0,
        ..card(15_000)
    };
    assert_eq!(
        resolve(&many, &auto_defaults(), None, Some(&d), &unknown)
            .fine
            .threads,
        Some(64)
    );
    // Save memory's own -fa on wins over Fast attention.
    let saving = ModelLoadSettings {
        save_memory: Toggle::On,
        ..fine(FineControl {
            fast_attention: FastAttention::Off,
            ..FineControl::default()
        })
    };
    let r = resolve(&saving, &auto_defaults(), None, Some(&d), &on_card);
    assert_eq!(r.fine.flash_attn, runtime::FlashAttn::Auto);
    assert_eq!(
        placement_words(&r, dev),
        "--device Vulkan0 -ngl all --fit off -fa on -ctk q8_0 -ctv q8_0"
    );
    // The resolver never produces an expert count of 0 (a defence: launch.rs adds no flag
    // for it either).
    let zero = FineControl {
        experts: ExpertsChoice::FirstLayers(0),
        ..FineControl::default()
    };
    assert_eq!(
        resolve_fine(&zero, GpuUse::All, false, Some(&m), &on_card).experts_on_cpu,
        runtime::ExpertsOnCpu::None
    );
}

#[test]
fn same_launch_sees_fine_control() {
    let d = dense();
    let m = card(15_000);
    let r = |o: &ModelLoadSettings| resolve(o, &auto_defaults(), None, Some(&d), &m);
    let base = r(&ModelLoadSettings::default());
    for (what, f) in [
        (
            "threads",
            FineControl {
                threads: ThreadsChoice::Count(4),
                ..FineControl::default()
            },
        ),
        (
            "layers",
            FineControl {
                layers: LayersChoice::Count(30),
                ..FineControl::default()
            },
        ),
        (
            "conversation memory",
            FineControl {
                kv: KvPlace::ComputerMemory,
                ..FineControl::default()
            },
        ),
        (
            "fast attention",
            FineControl {
                fast_attention: FastAttention::On,
                ..FineControl::default()
            },
        ),
    ] {
        assert!(!r(&fine(f)).same_launch(&base), "{what}");
    }
    // Leave free with As much as fits launches differently.
    let fit = ModelLoadSettings {
        gpu: GpuChoice::Fit,
        ..ModelLoadSettings::default()
    };
    let fit_leave = ModelLoadSettings {
        fine: FineControl {
            leave_free: LeaveFree::Mib(512),
            ..FineControl::default()
        },
        ..fit.clone()
    };
    assert!(!r(&fit_leave).same_launch(&r(&fit)));
}

#[test]
fn a_change_that_launches_the_same_is_not_settings_changed() {
    let d = dense();
    let m = card(15_000);
    let r = |o: &ModelLoadSettings| resolve(o, &auto_defaults(), None, Some(&d), &m);
    let leave = FineControl {
        leave_free: LeaveFree::Mib(2048),
        ..FineControl::default()
    };
    let layers = FineControl {
        layers: LayersChoice::Count(30),
        ..FineControl::default()
    };
    let fast_off = FineControl {
        fast_attention: FastAttention::Off,
        ..FineControl::default()
    };
    let every_layer = FineControl {
        layers: LayersChoice::Count(41),
        ..FineControl::default()
    };
    let off = ModelLoadSettings {
        gpu: GpuChoice::Off,
        ..ModelLoadSettings::default()
    };
    let saving = ModelLoadSettings {
        save_memory: Toggle::On,
        ..ModelLoadSettings::default()
    };
    for (what, before, after) in [
        (
            "Leave free with All of it",
            ModelLoadSettings::default(),
            fine(leave),
        ),
        (
            "layers with Don't use it",
            off.clone(),
            ModelLoadSettings {
                fine: layers,
                ..off.clone()
            },
        ),
        (
            "Fast attention with Save memory",
            saving.clone(),
            ModelLoadSettings {
                fine: fast_off,
                ..saving.clone()
            },
        ),
        (
            "every layer with All of it",
            ModelLoadSettings {
                gpu: GpuChoice::All,
                ..ModelLoadSettings::default()
            },
            ModelLoadSettings {
                gpu: GpuChoice::All,
                fine: every_layer,
                ..ModelLoadSettings::default()
            },
        ),
        (
            "every layer with All of it from the defaults",
            ModelLoadSettings::default(),
            fine(FineControl {
                layers: LayersChoice::Count(1000),
                ..FineControl::default()
            }),
        ),
    ] {
        let loaded = r(&before);
        let now = r(&after);
        assert!(now.same_launch(&loaded), "{what}");
        let info = build_info(
            "/m",
            &d,
            after.clone(),
            auto_defaults(),
            now,
            &m,
            Some(&loaded),
        );
        assert!(
            !info.older_settings_loaded,
            "{what}: no \"Settings changed\""
        );
    }
}

#[test]
fn automatic_with_a_custom_placement_and_with_leave_free() {
    let d = dense();
    // 6 GB free: the whole model leaves room for little conversation memory.
    let m = card(6_144);
    let auto = |o: ModelLoadSettings| {
        resolve(
            &ModelLoadSettings {
                context: ContextChoice::Auto,
                ..o
            },
            &auto_defaults(),
            None,
            Some(&d),
            &m,
        )
    };
    let plain = auto(ModelLoadSettings::default());
    assert_eq!(plain.ctx_choice, ContextSize::Auto);
    let input = d.fit_input(false).unwrap();
    assert_eq!(
        u64::from(plain.ctx),
        models::fit::auto_ctx(&input, m.room(GpuUse::All))
            .unwrap()
            .ctx,
        "without Fine control, fit.rs's Automatic, unchanged"
    );
    // 20 of 41 layers on the card: the largest size whose split fits.
    let custom = auto(fine(FineControl {
        layers: LayersChoice::Count(20),
        ..FineControl::default()
    }));
    let placer = Placer::new(&d, &m, false).unwrap();
    assert_eq!(
        u64::from(custom.ctx),
        placer.auto(GpuUse::All, &custom.fine).unwrap().ctx
    );
    assert!(custom.ctx > plain.ctx, "{} > {}", custom.ctx, plain.ctx);
    assert_eq!(custom.ctx_source, ContextSource::Automatic);
    // Conversation memory in computer memory: the same rule.
    let kv = auto(fine(FineControl {
        kv: KvPlace::ComputerMemory,
        ..FineControl::default()
    }));
    assert_eq!(
        u64::from(kv.ctx),
        placer.auto(GpuUse::All, &kv.fine).unwrap().ctx
    );
    assert!(kv.ctx > plain.ctx);
    // As much as fits with Leave free: fit.rs's Automatic leaving that much free.
    let leave = auto(ModelLoadSettings {
        gpu: GpuChoice::Fit,
        ..fine(FineControl {
            leave_free: LeaveFree::Mib(512),
            ..FineControl::default()
        })
    });
    assert_eq!(
        u64::from(leave.ctx),
        models::fit::auto_ctx_with_margin(&input, m.room(GpuUse::Fit), 512 * MIB)
            .unwrap()
            .ctx
    );
    assert!(leave.ctx > plain.ctx);
    // With All of it, Leave free changes nothing.
    let ignored = auto(fine(FineControl {
        leave_free: LeaveFree::Mib(512),
        ..FineControl::default()
    }));
    assert_eq!(ignored.ctx, plain.ctx);
}

#[test]
fn marks_follow_fine_control() {
    let d = dense();
    let m = card(6_144);
    let info = |o: ModelLoadSettings| {
        let r = resolve(&o, &auto_defaults(), None, Some(&d), &m);
        build_info("/m", &d, o, auto_defaults(), r, &m, None)
    };
    let mark_of = |i: &ModelLoadInfo, n: u32| {
        i.context_options
            .iter()
            .find(|o| o.choice == ContextSize::Fixed(n))
            .unwrap()
            .fit
    };
    let all = info(ModelLoadSettings {
        context: ContextChoice::Fixed(32_768),
        ..ModelLoadSettings::default()
    });
    assert_eq!(mark_of(&all, 32_768), Verdict::Yellow);
    assert_eq!(mark_of(&all, 4096), Verdict::Green);
    assert_eq!(all.fit, Some(Verdict::Yellow));
    assert_eq!(
        (all.picture.mark, all.picture.fits),
        (Verdict::Yellow, Fits::WontFitGraphics)
    );
    assert_eq!(
        all.gpu_options[0].fit,
        all.fit.unwrap(),
        "the selected option"
    );
    // Fewer layers in the draft: the marks follow.
    let twenty = info(ModelLoadSettings {
        context: ContextChoice::Fixed(32_768),
        ..fine(FineControl {
            layers: LayersChoice::Count(20),
            ..FineControl::default()
        })
    });
    assert_eq!(mark_of(&twenty, 32_768), Verdict::Green);
    assert_eq!(twenty.fit, Some(twenty.picture.mark), "the picture's mark");
    assert_eq!(twenty.picture.fits, Fits::Fits);
    assert!(twenty.picture.custom);
    assert_eq!(
        (twenty.picture.units, twenty.picture.layers_on_graphics),
        (Some(41), 20)
    );
    // The plain options are marked as choosing them would load (no custom placement).
    assert_eq!(
        twenty.gpu_options.iter().map(|o| o.fit).collect::<Vec<_>>(),
        all.gpu_options.iter().map(|o| o.fit).collect::<Vec<_>>()
    );
    // Too large: the floor does not fit, by the same verdict.
    let tiny = card(512);
    let small = Machine {
        ram_free_bytes: 3 * GB,
        ..tiny
    };
    let r = resolve(
        &ModelLoadSettings::default(),
        &auto_defaults(),
        None,
        Some(&d),
        &small,
    );
    let i = build_info(
        "/m",
        &d,
        ModelLoadSettings::default(),
        auto_defaults(),
        r,
        &small,
        None,
    );
    assert!(i.too_large);
    assert_eq!(i.picture.fits, Fits::WontFitAtAll);
    assert_eq!(i.context_options[0].fit, Verdict::Red);
}

#[test]
fn facts_without_a_tensor_table_keep_wp1b_marks() {
    let m = card(6_144);
    let no_sizes = ModelFacts {
        layers: Some(LayerFacts {
            sizes: None,
            ..dense().layers.unwrap()
        }),
        ..dense()
    };
    let no_layers = ModelFacts {
        layers: None,
        ..dense()
    };
    for f in [no_sizes, no_layers] {
        let r = resolve(
            &ModelLoadSettings::default(),
            &auto_defaults(),
            None,
            Some(&f),
            &m,
        );
        let info = build_info(
            "/m",
            &f,
            ModelLoadSettings::default(),
            auto_defaults(),
            r.clone(),
            &m,
            None,
        );
        let input = f.fit_input(false).unwrap();
        let room = m.room(GpuUse::All);
        for o in &info.context_options[1..] {
            assert_eq!(
                o.fit,
                models::fit::verdict_at(&input, u64::from(o.tokens), room),
                "{}",
                o.tokens
            );
        }
        let auto = models::fit::auto_ctx(&input, room).unwrap();
        assert_eq!(info.context_options[0].fit, auto.verdict);
        assert_eq!(u64::from(r.ctx), auto.ctx);
        assert_eq!(
            info.picture.source,
            PictureSource::Estimate {
                why: EstimateWhy::NotReadable,
                backend: None
            }
        );
        assert_eq!(info.picture.units, None);
        assert_eq!(info.picture.mark, info.fit.unwrap());
    }
}

#[test]
fn fine_facts_say_what_fine_control_may_offer() {
    let d = fine_facts(&dense(), &card(15_000));
    assert_eq!(
        d,
        FineFacts {
            units: Some(41),
            expert_layers: None,
            threads: 16,
            graphics: true,
            apple: false,
            graphics_limit_bytes: None,
        }
    );
    assert_eq!(fine_facts(&moe(), &card(15_000)).expert_layers, Some(48));
    let cpu = fine_facts(&dense(), &processor_only());
    assert!(!cpu.graphics && !cpu.apple);
    let apple = fine_facts(&dense(), &mac());
    assert!(apple.graphics && apple.apple);
    assert_eq!(apple.graphics_limit_bytes, Some(10_922 * MIB));
    let unreadable = ModelFacts {
        header: None,
        layers: None,
        ..dense()
    };
    assert_eq!(fine_facts(&unreadable, &card(15_000)).units, None);
    // Don't use it in the defaults still shows the graphics controls (greyed, with a note).
    let off = LocalModelSettings {
        gpu: GpuUse::Off,
        ..auto_defaults()
    };
    let m = card(15_000);
    let r = resolve(
        &ModelLoadSettings::default(),
        &off,
        None,
        Some(&dense()),
        &m,
    );
    let info = build_info(
        "/m",
        &dense(),
        ModelLoadSettings::default(),
        off,
        r,
        &m,
        None,
    );
    assert!(info.fine_facts.graphics);
    assert_eq!(info.fine_facts, fine_facts(&dense(), &m));
}

/// A graphics card's description from the probe goes through the one rule of item 6 2.0
/// wherever the panel shows it: in "Which graphics card" (`devices`) as in the picture's
/// graphics bar. Its name, which is only ever matched, stays as the probe wrote it.
#[test]
fn device_descriptions_from_the_probe_are_escaped() {
    let raw = "AMD\u{202E}Radeon\u{200B} RX\u{7}";
    let long = format!("Card {}", "x".repeat(200));
    let m = Machine::from_probes(
        &[
            probe(
                runtime::BackendKind::Vulkan,
                &[
                    ("Vulkan0", raw, 16_304, 15_000),
                    ("Vulkan1", long.as_str(), 8_000, 7_000),
                ],
            ),
            probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)]),
        ],
        None,
        Some("Vulkan0"),
        32 * GB,
        &Freed::default(),
    );
    assert_eq!(m.devices[0].name, "Vulkan0");
    assert_eq!(
        m.devices[0].description,
        "AMD[U+202E]Radeon[U+200B] RX[U+0007]"
    );
    assert_eq!(m.devices[0].description, m.gpu_description);
    assert_eq!(m.devices[1].description, usage::escape_text(&long, 80));
    assert!(m.devices[1].description.chars().count() <= 80);
    // The device is still chosen by its name.
    assert_eq!(
        validate(
            &ModelLoadSettings {
                device: DeviceChoice::Named("Vulkan0".into()),
                ..ModelLoadSettings::default()
            },
            None,
            &m
        ),
        Ok(())
    );
}

// ---- Files on disk -------------------------------------------------------------------------

/// A folder of its own, removed afterwards.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("fine-tests-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A small llama header with `blocks` blocks, `split.count` when given, and `tensors`.
fn gguf(blocks: u32, split_count: Option<u32>, tensors: &[(&str, u64)]) -> Vec<u8> {
    let mut w = models::gguf::testing::Writer::new()
        .str("general.architecture", "llama")
        .u32("llama.context_length", 8192)
        .u32("llama.block_count", blocks)
        .u32("llama.embedding_length", 256)
        .u32("llama.attention.head_count", 4)
        .u32("llama.attention.head_count_kv", 2);
    if let Some(n) = split_count {
        w = w.u32("split.count", n);
    }
    for (name, bytes) in tensors {
        w = w.tensor(name, *bytes);
    }
    w.build()
}

/// Section 4.2 step 2: a split model's other files are found only from the first file's
/// own name, at most 256 of them and as many as its header's `split.count`; one missing
/// or unreadable part, or another part than the first, leaves the sizes unknown (the
/// picture is then `notReadable`), while the header facts stay.
#[tokio::test]
async fn split_parts_are_found_only_from_the_first_files_name() {
    let write_set = |dir: &std::path::Path, stem: &str, count: u32, parts: &[u32]| {
        let tables: [&[(&str, u64)]; 3] = [
            &[("token_embd.weight", 64), ("blk.0.attn_q.weight", 96)],
            &[("blk.1.attn_q.weight", 96)],
            &[("output.weight", 64)],
        ];
        for &i in parts {
            let bytes = gguf(2, Some(count), tables[(i - 1) as usize]);
            std::fs::write(dir.join(format!("{stem}-{i:05}-of-{count:05}.gguf")), bytes).unwrap();
        }
    };
    // Every part there: the tables are summed.
    let whole = Scratch::new();
    write_set(&whole.0, "big", 3, &[1, 2, 3]);
    let first = whole.0.join("big-00001-of-00003.gguf");
    let f = read_facts(&first, None).await.unwrap();
    let sizes = f
        .layers
        .as_ref()
        .unwrap()
        .sizes
        .clone()
        .expect("every part read");
    assert_eq!(sizes.layers.len(), 2);
    assert_eq!(sizes.input_bytes, 64);
    assert_eq!(sizes.output_bytes, 64);
    assert_eq!(sizes.layers[0].dense_bytes, 96);
    assert_eq!(sizes.layers[1].dense_bytes, 96);
    assert_eq!(f.layers.as_ref().unwrap().units(), 3);
    // Opened from another part: not found.
    let second = whole.0.join("big-00002-of-00003.gguf");
    let f = read_facts(&second, None).await.unwrap();
    assert!(f.header.is_some());
    assert_eq!(f.layers.unwrap().sizes, None);
    // One part missing: unknown sizes, header facts kept, the picture says so.
    let gap = Scratch::new();
    write_set(&gap.0, "big", 3, &[1, 2]);
    let f = read_facts(&gap.0.join("big-00001-of-00003.gguf"), None)
        .await
        .unwrap();
    assert!(f.header.is_some());
    let layers = f.layers.clone().unwrap();
    assert_eq!((layers.sizes.clone(), layers.units()), (None, 3));
    let m = processor_only();
    let r = resolve(
        &ModelLoadSettings::default(),
        &auto_defaults(),
        None,
        Some(&f),
        &m,
    );
    let info = build_info(
        "/m",
        &f,
        ModelLoadSettings::default(),
        auto_defaults(),
        r,
        &m,
        None,
    );
    assert_eq!(
        info.picture.source,
        PictureSource::Estimate {
            why: EstimateWhy::NotReadable,
            backend: None
        }
    );
    // The name and the header disagree about the count.
    let odd = Scratch::new();
    write_set(&odd.0, "big", 3, &[1, 2, 3]);
    std::fs::write(
        odd.0.join("big-00001-of-00003.gguf"),
        gguf(
            2,
            Some(2),
            &[("token_embd.weight", 64), ("blk.0.attn_q.weight", 96)],
        ),
    )
    .unwrap();
    let f = read_facts(&odd.0.join("big-00001-of-00003.gguf"), None)
        .await
        .unwrap();
    assert_eq!(f.layers.unwrap().sizes, None);
    // More than 256 parts: never looked for.
    assert_eq!(
        other_split_files("x-00001-of-00300.gguf", Some(300)),
        Err(())
    );
    assert_eq!(
        other_split_files("x-00001-of-00256.gguf", Some(256)).map(|v| v.len()),
        Ok(255)
    );
    assert_eq!(
        other_split_files("x-00001-of-00002.gguf", Some(2)),
        Ok(vec!["x-00002-of-00002.gguf".to_string()])
    );
    assert_eq!(other_split_files("x.gguf", None), Ok(Vec::new()));
    assert_eq!(other_split_files("x.gguf", Some(2)), Err(()));
    assert_eq!(other_split_files("x-00001-of-00002.gguf", None), Err(()));
    // A single file: its own table.
    let one = Scratch::new();
    let path = one.0.join("one.gguf");
    std::fs::write(
        &path,
        gguf(
            2,
            None,
            &[
                ("token_embd.weight", 64),
                ("blk.0.attn_q.weight", 96),
                ("blk.1.attn_q.weight", 96),
                ("blk.1.ffn_up_exps.weight", 320),
            ],
        ),
    )
    .unwrap();
    let f = read_facts(&path, None).await.unwrap();
    let layers = f.layers.unwrap();
    assert!(layers.experts, "expert parts in the table");
    assert_eq!(layers.expert_layers(), Some(2));
    assert_eq!(layers.sizes.unwrap().layers[1].expert_bytes, 320);
}

/// The real small model (`TEST_GGUF`): its tensor table is read, its units are its blocks
/// and the output unit, and its picture is the instant estimate.
#[tokio::test]
async fn the_real_model_has_layer_facts() {
    let Ok(path) = std::env::var("TEST_GGUF") else {
        eprintln!("TEST_GGUF not set: skipped");
        return;
    };
    let path = std::path::PathBuf::from(path);
    let f = read_facts(&path, None).await.expect("the file is there");
    let header = models::read_file(&path).unwrap();
    let summary = models::ModelSummary::from_header(&header);
    let layers = f.layers.clone().expect("layer facts");
    let sizes = layers.sizes.clone().expect("the tensor table is read");
    assert_eq!(u64::from(layers.units()), summary.block_count + 1);
    assert!(sizes.total() > f.size_bytes / 2);
    let m = processor_only();
    let r = resolve(
        &ModelLoadSettings::default(),
        &auto_defaults(),
        None,
        Some(&f),
        &m,
    );
    let info = build_info(
        &path.display().to_string(),
        &f,
        ModelLoadSettings::default(),
        auto_defaults(),
        r,
        &m,
        None,
    );
    assert_eq!(
        info.picture.source,
        PictureSource::Estimate {
            why: EstimateWhy::Quick,
            backend: None
        }
    );
    assert_eq!(info.picture.graphics, None);
    assert_eq!(info.picture.computer.need.model, sizes.total());
    assert_eq!(info.fine_facts.units, Some(layers.units()));
    assert!(!info.fine_facts.graphics);
}
