//! Contract tests for the Studio engine: the pinned `sd-cli` knows every flag and word
//! Studio sends, parses the exact arguments `build_args` makes, and lists its devices in the
//! form `parse_list_devices` reads (docs/dev/0.1.0/studio-local-design.md, L2).
//!
//! They read `SD_CLI_BIN` (CI's `checks` job builds a processor-only `sd-cli` from the
//! pinned commit) and print "skipped" when it is unset. Nothing here loads a model: the
//! arguments are checked in metadata mode (`-M metadata --image <png>` after them, the last
//! `-M` winning), which parses every option and reads only the picture.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use runtime::sd::args::{
    FLAGS, HighNoise, LoraTag, OutputFormat, PartRole, Sampler, Scheduler, SdJob, VaeFormat,
    build_args, missing_from_help, negative_file_text, prompt_file_text, prompt_files,
};
use runtime::sd::devices::parse_list_devices;
use runtime::{SpawnSpec, spawn};
use tokio::io::AsyncReadExt;

/// A 1×1 PNG: the start frame, the reference picture and the picture metadata mode reads.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

/// How long one `sd-cli` run may take: none of them loads a model.
const RUN_BOUND: Duration = Duration::from_secs(60);

fn engine(test: &str) -> Option<PathBuf> {
    let bin = std::env::var_os("SD_CLI_BIN").filter(|v| !v.is_empty());
    if bin.is_none() {
        eprintln!("skipped {test}: set SD_CLI_BIN to the pinned sd-cli to run it");
    }
    bin.map(PathBuf::from)
}

/// Runs `bin` with `args` the way Studio starts it (through `runtime::spawn`, in the
/// engine's folder) and returns its exit status, stdout and stderr.
async fn run(bin: &Path, args: Vec<OsString>) -> (ExitStatus, String, String) {
    let dir = bin.parent().expect("SD_CLI_BIN has a parent directory");
    let mut child = spawn(SpawnSpec::new(bin, dir).args(args)).expect("start sd-cli");
    let mut stdout = child.take_stdout().expect("stdout piped");
    let mut stderr = child.take_stderr().expect("stderr piped");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let status = tokio::time::timeout(RUN_BOUND, async {
        let (read_out, read_err) =
            tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        read_out.expect("read stdout");
        read_err.expect("read stderr");
        child.wait().await.expect("wait for sd-cli")
    })
    .await
    .expect("sd-cli finishes within the bound");
    (
        status,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    )
}

#[tokio::test]
async fn the_pinned_engine_knows_every_flag_studio_sends() {
    let Some(bin) = engine("the_pinned_engine_knows_every_flag_studio_sends") else {
        return;
    };
    let (status, help, stderr) = run(&bin, vec!["-h".into()]).await;
    assert!(status.success(), "{status}: {stderr}");
    assert_eq!(missing_from_help(&help), Vec::<&str>::new(), "{help}");
    for flag in FLAGS {
        assert!(help.contains(flag), "{flag}");
    }
}

#[tokio::test]
async fn the_pinned_engine_parses_the_exact_arguments_studio_builds() {
    let Some(bin) = engine("the_pinned_engine_parses_the_exact_arguments_studio_builds") else {
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    let picture_png = scratch.path().join("tiny.png");
    std::fs::write(&picture_png, TINY_PNG).unwrap();
    let models = scratch.path().join("models");
    let picture = SdJob {
        parts: vec![
            (
                PartRole::Diffusion,
                models.join("flux-2-klein-4b-Q4_K_M.gguf"),
            ),
            (PartRole::Llm, models.join("Qwen3-4B-Q4_K_M.gguf")),
            (PartRole::Vae, models.join("flux2_ae.safetensors")),
        ],
        prompt: "-o /etc/x a red kite over the sea".into(),
        negative: Some("--vae blurry".into()),
        width: 1024,
        height: 768,
        size_multiple: 16,
        steps: 4,
        cfg: 1.0,
        guidance: None,
        img_cfg: None,
        flow_shift: None,
        timestep_shift: None,
        sampler: Some(Sampler::Euler),
        scheduler: Some(Scheduler::Simple),
        sigmas: Vec::new(),
        high_noise: None,
        moe_boundary: None,
        strength: None,
        seed: 2_147_483_647,
        batch: 2,
        frames: None,
        fps: None,
        init_image: None,
        end_image: None,
        mask_image: None,
        control_image: None,
        ref_images: vec![picture_png.clone()],
        vae_format: None,
        model_args: None,
        lora_dir: None,
        loras: Vec::new(),
        backend: None,
        vae_tiling: true,
        offload_to_cpu: true,
        tiling_allowed: true,
        processor_only: true,
        threads: Some(3),
        out_dir: scratch.path().join("picture"),
        out_stem: "r1".into(),
        format: OutputFormat::Png,
        env: Vec::new(),
    };
    let video = SdJob {
        parts: vec![
            (
                PartRole::Diffusion,
                models.join("Wan2.2-TI2V-5B-Q4_K_M.gguf"),
            ),
            (PartRole::T5xxl, models.join("umt5-xxl-encoder-Q8_0.gguf")),
            (PartRole::Vae, models.join("Wan2.2_VAE.safetensors")),
        ],
        prompt: "a river at dawn".into(),
        negative: None,
        width: 832,
        height: 480,
        steps: 8,
        cfg: 5.0,
        flow_shift: Some(3.0),
        sampler: Some(Sampler::DpmPp2mV2),
        scheduler: None,
        seed: 0,
        batch: 1,
        frames: Some(49),
        fps: Some(24),
        init_image: Some(picture_png.clone()),
        ref_images: Vec::new(),
        processor_only: false,
        threads: None,
        vae_tiling: false,
        offload_to_cpu: false,
        out_dir: scratch.path().join("video"),
        out_stem: "r2".into(),
        format: OutputFormat::Webm,
        ..picture.clone()
    };
    for job in [picture, video] {
        parses_in_metadata_mode(&bin, &job, &picture_png).await;
    }
}

/// Writes `job`'s prompt files as `run_job` does, runs `bin` with `build_args(job)` then
/// `-M metadata --image <picture>` (the last `-M` wins), and checks that the engine parsed
/// every argument: exit 0, and no `unknown argument` or `invalid` line.
async fn parses_in_metadata_mode(bin: &Path, job: &SdJob, picture: &Path) {
    std::fs::create_dir_all(&job.out_dir).unwrap();
    let (prompt_file, negative_file) = prompt_files(job);
    std::fs::write(&prompt_file, prompt_file_text(job)).unwrap();
    if let (Some(file), Some(negative)) = (&negative_file, negative_file_text(job)) {
        std::fs::write(file, negative).unwrap();
    }
    let mut args = build_args(job).unwrap();
    args.extend([
        "-M".into(),
        "metadata".into(),
        "--image".into(),
        picture.as_os_str().to_owned(),
    ]);
    let (status, stdout, stderr) = run(bin, args).await;
    let output = format!("{stdout}{stderr}");
    assert!(status.success(), "{status} for {job:?}: {output}");
    let lower = output.to_lowercase();
    assert!(
        !lower.contains("unknown argument") && !lower.contains("invalid"),
        "{job:?}: {output}"
    );
}

#[tokio::test]
async fn the_pinned_engine_parses_recipe_arguments() {
    let Some(bin) = engine("the_pinned_engine_parses_recipe_arguments") else {
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    let png = |name: &str| {
        let path = scratch.path().join(name);
        std::fs::write(&path, TINY_PNG).unwrap();
        path
    };
    let (picture_png, start, end, mask, edges) = (
        png("tiny.png"),
        png("start.png"),
        png("end.png"),
        png("mask.png"),
        png("edges.png"),
    );
    let models = scratch.path().join("models");
    let loras = models.join("@loras");
    std::fs::create_dir_all(loras.join("local")).unwrap();
    std::fs::write(loras.join("local").join("pixel art ü.safetensors"), b"lora").unwrap();
    // Wan 2.2 T2V A14B with every recipe field set, a LoRA on each part
    let wan_a14b = SdJob {
        parts: vec![
            (
                PartRole::Diffusion,
                models.join("wan2.2_t2v_low_noise_14B_Q4_K_M.gguf"),
            ),
            (
                PartRole::HighNoiseDiffusion,
                models.join("wan2.2_t2v_high_noise_14B_Q4_K_M.gguf"),
            ),
            (PartRole::T5xxl, models.join("umt5-xxl-encoder-Q8_0.gguf")),
            (PartRole::Vae, models.join("wan_2.1_vae.safetensors")),
        ],
        prompt: "a river at dawn <lora:typed:3>".into(),
        negative: Some("blurry".into()),
        width: 832,
        height: 480,
        size_multiple: 16,
        steps: 20,
        cfg: 3.0,
        guidance: Some(3.5),
        img_cfg: Some(1.5),
        flow_shift: Some(12.0),
        timestep_shift: Some(250),
        sampler: Some(Sampler::Euler),
        scheduler: Some(Scheduler::Simple),
        sigmas: vec![1.0, 0.75, 0.5, 0.25, 0.0],
        high_noise: Some(HighNoise {
            steps: -1,
            cfg: 4.0,
            sampler: Some(Sampler::Euler),
        }),
        moe_boundary: Some(0.875),
        strength: Some(0.75),
        seed: 42,
        batch: 1,
        frames: Some(33),
        fps: Some(16),
        init_image: Some(start),
        end_image: Some(end),
        mask_image: None,
        control_image: None,
        ref_images: Vec::new(),
        vae_format: Some(VaeFormat::Wan),
        model_args: Some("qwen_image_zero_cond_t=true".into()),
        lora_dir: Some(loras.clone()),
        loras: vec![
            LoraTag {
                name: "local/pixel art ü".into(),
                weight: 0.8,
                high_noise: false,
            },
            LoraTag {
                name: "local/pixel art ü".into(),
                weight: 1.0,
                high_noise: true,
            },
        ],
        backend: None,
        vae_tiling: true,
        offload_to_cpu: true,
        tiling_allowed: true,
        processor_only: true,
        threads: Some(2),
        out_dir: scratch.path().join("video"),
        out_stem: "r1".into(),
        format: OutputFormat::Webm,
        env: Vec::new(),
    };
    // a picture to change with its mask, and a control picture
    let inpaint = SdJob {
        parts: vec![
            (PartRole::Diffusion, models.join("flux1-fill-dev-Q4_0.gguf")),
            (PartRole::ClipL, models.join("clip_l.safetensors")),
            (PartRole::T5xxl, models.join("t5xxl_fp16.safetensors")),
            (PartRole::Vae, models.join("ae.safetensors")),
        ],
        prompt: "a red kite".into(),
        negative: None,
        width: 1024,
        height: 1024,
        size_multiple: 8,
        steps: 20,
        cfg: 1.0,
        guidance: Some(30.0),
        img_cfg: None,
        flow_shift: None,
        timestep_shift: None,
        sigmas: Vec::new(),
        high_noise: None,
        moe_boundary: None,
        strength: Some(1.0),
        frames: None,
        fps: None,
        init_image: Some(picture_png.clone()),
        end_image: None,
        mask_image: Some(mask),
        control_image: Some(edges),
        vae_format: Some(VaeFormat::Flux),
        model_args: None,
        lora_dir: None,
        loras: Vec::new(),
        vae_tiling: false,
        offload_to_cpu: false,
        tiling_allowed: false,
        out_dir: scratch.path().join("picture"),
        out_stem: "r2".into(),
        format: OutputFormat::Png,
        ..wan_a14b.clone()
    };
    for job in [wan_a14b, inpaint] {
        parses_in_metadata_mode(&bin, &job, &picture_png).await;
    }
}

#[tokio::test]
async fn the_pinned_engine_lists_its_devices_in_the_format_studio_reads() {
    let Some(bin) = engine("the_pinned_engine_lists_its_devices_in_the_format_studio_reads") else {
        return;
    };
    let (status, stdout, stderr) = run(&bin, vec!["--list-devices".into()]).await;
    assert!(status.success(), "{status}: {stderr}");
    let devices = parse_list_devices(&stdout);
    assert!(
        devices
            .iter()
            .any(|device| device.name.eq_ignore_ascii_case("cpu")),
        "{devices:?} from {stdout:?}"
    );
    assert_eq!(devices.len(), stdout.lines().count(), "{stdout:?}");
}
