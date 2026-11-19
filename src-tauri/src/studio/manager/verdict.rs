//! A local run's verdict (Task L15, ruling L15-x-L16): where a run of a model at one size
//! would go on this computer as it is now, its fit and its time there, its word, and what the
//! graphics card's lease is asked for ([`local_verdict`]). Every fit Studio shows for a local
//! model is this one: the local lane's start, estimate, preview and task, and every fit Get
//! local models shows (Task L16), so Create and Get local models say the same. Also how a
//! device is named as a run records it, and how a time and a fit word are shown.

use models::fit::Target;
use models::gen_catalog::{GenEntry, GenKind};
use models::gen_eta::{self, Eta, FitWord, MeasuredRun};
use models::gen_fit::{GenFit, GenFitResult, GenSize, gen_fit};
use serde_json::{Value, json};

use crate::dto::EtaDto;
use crate::gpu_lease::StudioNeed;
use crate::studio::deps::{LocalHardware, with_device_free};
use crate::studio::errors;
use crate::studio::local::RunShape;
use crate::studio::prefs::GpuPolicy;

/// The device a run on the processor records, as shown.
pub(super) const PROCESSOR: &str = "Processor";

/// How many of this computer's measured runs of a model an estimate reads.
const MEASURED_RUNS: u32 = 10;

/// A run of a model at one size on this computer: where it would run, its fit and its time
/// there, its word, and what the graphics card's lease is asked for.
#[derive(Debug, Clone)]
pub(crate) struct LocalVerdict {
    /// Its fit where it would run: on the graphics device (or on the processor of a computer
    /// that runs models there), or on the processor for a picture that does not fit the device
    /// even with the chat model unloaded (`processor_only` is then set).
    pub fit: GenFitResult,
    /// How long it takes there: from this computer's runs measured on that device, else the
    /// catalog's first guess; `None` with neither.
    pub eta: Option<Eta>,
    /// The word its fit gets (`gen_eta::fit_word`).
    pub word: FitWord,
    /// The graphics device as shown ("Processor" on a computer that runs models there): what a
    /// run the grant leaves on the card records.
    pub card: String,
    /// llama.cpp's description of the graphics device, for the engine's `--backend`.
    pub description: Option<String>,
    /// Why not, when the word is `NotHere` (`errors::not_here_reason`).
    pub reason: Option<String>,
    /// What the lease is asked for.
    pub need: StudioNeed,
}

/// `entry` at `size`, `steps` and `pictures` on this computer as it is now, judged where the
/// run would go: [`LocalVerdict`]. Every fit Studio shows for a local model is this one, so
/// Create and Get local models say the same. The measured times read are those of the device
/// the run would use, as a run records it ("<device> (<backend>)", or "Processor").
pub(crate) async fn local_verdict(
    entry: &GenEntry,
    size: GenSize,
    steps: u32,
    pictures: u32,
    hardware: &dyn LocalHardware,
    store: &store::Store,
) -> LocalVerdict {
    let shape = RunShape {
        size,
        steps,
        pictures,
    };
    verdict(entry, shape, false, hardware, store).await
}

/// [`local_verdict`], for a run the request puts on the processor when `by_choice` (the
/// `run_on` field).
pub(super) async fn verdict(
    entry: &GenEntry,
    shape: RunShape,
    by_choice: bool,
    hardware: &dyn LocalHardware,
    store: &store::Store,
) -> LocalVerdict {
    let RunShape {
        size,
        steps,
        pictures,
    } = shape;
    let (target, backend, description) = hardware.target().await;
    let chat = hardware.chat_device_bytes();
    let on_device = gen_fit(entry, size, &target, chat);
    let processor = Target::Cpu {
        ram_free_bytes: free_memory(&target),
    };
    let on_processor = gen_fit(entry, size, &processor, None);
    let processor_ok = entry.kind != GenKind::Video && on_processor.fit != GenFit::Red;
    let shared_memory = matches!(target, Target::AppleUnified { .. });
    let card = if on_device.processor_only {
        PROCESSOR.to_owned()
    } else {
        shown(description.as_deref(), &backend)
    };
    // The processor when the request asks for it, on a computer that runs models there, and
    // for a picture the device cannot take even with the chat model unloaded.
    let not_on_device = gen_eta::fit_word(entry, &on_device, None) == FitWord::NotHere;
    let runs_on_processor =
        by_choice || on_device.processor_only || (not_on_device && processor_ok);
    let (fit, device) = match (runs_on_processor, on_device.processor_only) {
        (false, _) => (on_device, card.as_str()),
        (true, true) => (on_device, PROCESSOR),
        (true, false) => (on_processor, PROCESSOR),
    };
    // The device's class is the card's own, whatever the chat model takes of it now.
    let class = if runs_on_processor {
        gen_eta::device_class(&target, "cpu")
    } else {
        gen_eta::device_class(&chat_counted_back(&target, chat), &backend)
    };
    let measured = match store
        .local_run_times(&entry.id, device, MEASURED_RUNS)
        .await
    {
        Ok(times) => measured_runs(&times),
        Err(e) => {
            tracing::warn!(model = %entry.id, "this computer's measured times could not be read: {e}");
            Vec::new()
        }
    };
    let eta = gen_eta::estimate(entry, size, steps, pictures, &class, &measured);
    let word = gen_eta::fit_word(entry, &fit, eta.as_ref());
    let reason = fit
        .not_for
        .filter(|_| word == FitWord::NotHere)
        .map(|not_for| errors::not_here_reason(not_for, fit.processor_only));
    let need = StudioNeed {
        judged_with: chat,
        alongside: on_card(on_device.fit) || (by_choice && !shared_memory),
        after_unload: on_device.after_unload.is_some_and(|f| f != GenFit::Red),
        processor_ok,
        shared_memory,
    };
    LocalVerdict {
        fit,
        eta,
        word,
        card,
        description,
        reason,
        need,
    }
}

/// Whether the lease would run `verdict`'s run on the processor rather than unload the chat
/// model, as `GpuLease::acquire_for_studio` decides it: under "Use the processor for local
/// pictures" (`policy`), with a chat model on the device (`chat_loaded`), a run judged for
/// the device that fits there only once the chat model is unloaded and can run on the
/// processor (a picture). A clip is still made after the unload. ("Keep in memory" on a
/// Mac's shared memory unloads the chat model whatever the policy; an estimate cannot see
/// it.)
pub(super) fn processor_instead_of_unload(
    verdict: &LocalVerdict,
    policy: GpuPolicy,
    chat_loaded: bool,
) -> bool {
    let need = &verdict.need;
    policy == GpuPolicy::Processor
        && chat_loaded
        && !verdict.fit.processor_only
        && !need.alongside
        && need.after_unload
        && need.processor_ok
}

/// Whole or one part at a time on the device: nothing of it on the processor.
fn on_card(fit: GenFit) -> bool {
    matches!(fit, GenFit::Green | GenFit::OnePartAtATime)
}

/// `target` with the chat model's `chat` bytes on the device counted back as free.
fn chat_counted_back(target: &Target, chat: Option<u64>) -> Target {
    let back = chat.unwrap_or(0);
    with_device_free(target.clone(), |free| free.saturating_add(back))
}

/// The computer's free memory where the processor runs a model: 0 when the system does not
/// say.
fn free_memory(target: &Target) -> u64 {
    match target {
        Target::Gpu { ram_free_bytes, .. } | Target::AppleUnified { ram_free_bytes, .. } => {
            ram_free_bytes.unwrap_or(0)
        }
        Target::Cpu { ram_free_bytes } => *ram_free_bytes,
    }
}

/// A graphics device as shown: its name ([`device_label`]) with the engine's backend in
/// brackets ("AMD Radeon RX 7600 (Vulkan)", "Apple M5 (Metal)").
fn shown(description: Option<&str>, backend: &str) -> String {
    let label = device_label(description, backend);
    let backend = match backend {
        "cpu" => return label,
        "vulkan" => "Vulkan",
        "metal" => "Metal",
        other => other,
    };
    format!("{label} ({backend})")
}

/// Where models run, by name: the graphics device's ([`device_name`]; "Graphics card" when
/// llama.cpp gives none), or "Processor" for the backend `cpu`.
pub(crate) fn device_label(description: Option<&str>, backend: &str) -> String {
    if backend == "cpu" {
        return PROCESSOR.to_owned();
    }
    description
        .map(device_name)
        .filter(|name| !name.is_empty())
        .unwrap_or("Graphics card")
        .to_owned()
}

/// A graphics device's name from llama.cpp's description of it: the driver's code for the
/// chip that the description may end with, set apart by a space and in brackets (Mesa's
/// "AMD Radeon RX 7600 (RADV NAVI33)", Intel's "… Graphics (DG2)"), is left out. Brackets
/// inside the name ("AMD Radeon(TM) Graphics") are its own.
pub(crate) fn device_name(description: &str) -> &str {
    let name = description.trim();
    let Some(inner) = name.strip_suffix(')') else {
        return name;
    };
    let mut depth = 0_usize;
    for (at, c) in inner.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' if depth > 0 => depth -= 1,
            '(' => {
                let before = &name[..at];
                let kept = before.trim_end();
                return if before.len() > kept.len() && !kept.is_empty() {
                    kept
                } else {
                    name
                };
            }
            _ => {}
        }
    }
    name
}

/// This computer's measured runs, from what each recorded run was made at (its `job`).
fn measured_runs(times: &[store::LocalRunTime]) -> Vec<MeasuredRun> {
    times
        .iter()
        .filter_map(|time| {
            let job = time.params.get("job")?;
            let number = |key: &str| {
                job.get(key)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            Some(MeasuredRun {
                size: GenSize {
                    width: number("width")?,
                    height: number("height")?,
                    frames: number("frames").unwrap_or(1),
                },
                steps: number("steps")?,
                pictures: number("pictures").unwrap_or(1),
                elapsed_ms: time.elapsed_ms,
            })
        })
        .collect()
}

/// What a run made at `shape` records of it, for this computer's measured times.
pub(super) fn job_record(shape: &RunShape) -> Value {
    json!({
        "width": shape.size.width,
        "height": shape.size.height,
        "frames": shape.size.frames,
        "steps": shape.steps,
        "pictures": shape.pictures,
    })
}

/// A time as the webview shows it: rounded as people read it (`gen_eta::round_for_display`),
/// with the time left during a run.
pub(crate) fn eta_dto(eta: &Eta, left_ms: Option<u64>) -> EtaDto {
    EtaDto {
        low_ms: gen_eta::round_for_display(eta.low_ms),
        high_ms: gen_eta::round_for_display(eta.high_ms),
        measured: eta.measured,
        left_ms: left_ms.map(gen_eta::round_for_display),
    }
}

/// A fit word as the webview names it: `fits`, `slow`, `afterUnload` or `notHere`.
pub(crate) fn word_tag(word: FitWord) -> &'static str {
    match word {
        FitWord::Fits => "fits",
        FitWord::Slow => "slow",
        FitWord::AfterUnload => "afterUnload",
        FitWord::NotHere => "notHere",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ruling L15-x-B9-device: a device is shown by its name with the backend in brackets,
    /// never with the driver's code for its chip.
    #[test]
    fn a_device_is_shown_by_its_name_and_backend() {
        let vulkan = |description: &str| shown(Some(description), "vulkan");
        assert_eq!(
            vulkan("AMD Radeon RX 7600 (RADV NAVI33)"),
            "AMD Radeon RX 7600 (Vulkan)"
        );
        assert_eq!(
            vulkan("Intel(R) Arc(tm) A770 Graphics (DG2)"),
            "Intel(R) Arc(tm) A770 Graphics (Vulkan)"
        );
        assert_eq!(
            vulkan("llvmpipe (LLVM 17.0.6, 256 bits)"),
            "llvmpipe (Vulkan)"
        );
        assert_eq!(vulkan("Card (rev (a1))"), "Card (Vulkan)");
        // Brackets that are part of the name stay.
        assert_eq!(
            vulkan("NVIDIA GeForce RTX 4070"),
            "NVIDIA GeForce RTX 4070 (Vulkan)"
        );
        assert_eq!(
            vulkan("AMD Radeon(TM) Graphics"),
            "AMD Radeon(TM) Graphics (Vulkan)"
        );
        assert_eq!(vulkan("AMD Radeon(TM)"), "AMD Radeon(TM) (Vulkan)");
        // A description that is nothing but brackets is all there is to name it by.
        assert_eq!(vulkan("(RADV NAVI33)"), "(RADV NAVI33) (Vulkan)");
        assert_eq!(shown(Some("Apple M5"), "metal"), "Apple M5 (Metal)");
        assert_eq!(vulkan("  "), "Graphics card (Vulkan)");
        assert_eq!(shown(None, "vulkan"), "Graphics card (Vulkan)");
        assert_eq!(
            shown(Some("AMD Radeon RX 7600 (RADV NAVI33)"), "cpu"),
            PROCESSOR
        );
    }
}
