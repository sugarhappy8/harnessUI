# P2 (image files) independent review: findings to fix (round 1)

No critical problems. The owner's case is fixed (real-binary contract test passes).
To fix after P6b finishes (it edits the same download/image-file code). m5 was handed to P6b.

## Major
- **M1** An image file that failed for a reason that is not its own (a backend exception
  such as Vulkan `vk::...ErrorDeviceLost`, or a temporary open/read failure, all printed as
  `clip_init: failed to load model '<file>': <what>`, clip.cpp:5114) is classified
  `ImageFileInvalid`, remembered by name/size/mtime for every backend, and there is no way
  back in the app (`pick` never offers the same name and size; "Its repository has no other
  image file"). Fix: a "Try it again" action that clears the entry (main-window command);
  offer "Download the image file again" (same name/size when its SHA differs, or simply
  again); optionally don't remember when the message is a backend or I/O error
  (`vk::`, `CUDA error`, `failed to open`). Also failure.rs:81-83 advice contradicts this.
- **M2** The automatic Hugging Face listing for every downloaded model without a usable
  image file (most text-only models), whenever Models opens (Installed is the default tab),
  uncached failures, token sent, rate limit shared with Browse; SECURITY.md says 0.0.6
  network use is "only when you ask". Fix (decided: make it user-triggered): save the
  repository's image files (name, size, SHA-256) on the model's row at download time
  (`download_start` already re-reads the listing) and offer "Download the image file" from
  that saved data with zero requests; for models downloaded before 0.0.7 show a "Look for an
  image file" link that calls `image_file_offer` on click; asking automatically only after an
  image-file failure is acceptable; extend `e2e/m8_offline.py` to open Installed with a model
  that has a repository.

## Minor
- **m1** A damaged image file turns an unrelated model failure into "broken model file"
  (no step-down): the server reads the image file before loading the model, and ggml's GGUF
  reader prints `invalid magic characters` / `failed to open GGUF file`, which match the
  model-file signatures. Fix: only match model-file signatures after the last
  `mtmd_get_memory_usage` / `[mtmd] failed to get memory usage` line; test with the real
  capture plus a device error.
- **m2** A crash after Ready can drop the image file and remember it (the crash path reaches
  the image-file branch; only text echoed into the log can cause it, e.g.
  `task id = %d, error: %s`). Fix: skip the image-file branch on the crash path; test
  `Script::Crash(_, ImageFileInvalid)`.
- **m3** Untrusted text in log lines (architecture names, key/tensor names, paths) can
  change the classification (substring match anywhere). Fix: anchor signatures at the start
  of the message after timestamp/level; for the server line require the exact `--mmproj`
  path.
- **m4** In shared folders another model's image file gets picked (text-only models then
  carry a permanent warning; with more than 8 wrong files the cycle restarts; `pick` offers
  the smallest F16 whatever the model size). Fix: check the candidate's projection width
  equals the model's embedding length (llama.cpp's own check, mtmd.cpp:708-714); at least
  prefer names sharing the model's name and use an unrelated file only when it's the only
  one; no note when every skipped file belonged to another model.
- **m5** (handed to P6b) image-file download joins/resumes an unrelated group.
- **m6** Compare's "Can it read images?" uses the recorded file, not `image_file::choose`.
- **m7** Text: failure.rs:81-83 advice; "Download its image file in Models → Installed" is
  now told to every text-only model (suggest "If it has an image file, download it in
  Models → Installed; otherwise pick a model with the vision badge."); words.ts:236
  "Reload the model to use it" when not loaded (suggest "It will be used the next time the
  model loads."); ImageFile.tsx:133 "no other image file" when it has none.
- **m8** Tests: failure.rs:471-494 invents a log line the pinned build can't print; missing
  negative tests for m1, m2, M1, m3, m5, m4/m9.
- **m9** `beside` follows symlinks and files written next to a model now load automatically
  (under Allow all with the home folder approved, a prompt-injected model could write one
  with file tools). Fix: `symlink_metadata`, regular files only; make writes into the models
  folder always ask.
- **m10** Pre-existing: `validate_repo_path` misses COM¹–³/LPT¹–³ and "AUX .mmproj.gguf";
  bidi/zero-width characters pass though a comment claims otherwise; the offered file name
  is unescaped in the button tooltip.
- **m11** Design: running out of memory while the image file loads never falls back to
  loading without it (a last-resort retry without the image file, when the memory failure
  came after the model loaded).
