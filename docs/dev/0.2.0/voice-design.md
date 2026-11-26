<!-- Owner decisions recorded 2026-11-26 -->
> **The owner's words, verbatim.**
>
> - "Actually also once your done with this update then start planning for a voice model similar to image and video. Weights for the voices itself should be capable to train on the app or bring your own. I want it similar to how elevenlabs trains voice from users mp3 or wav"
> - "After ur done start the voice update straight away, on the same version/pull if u can"
> - Asked "what will you mainly use the voices for?", the owner answered: "All of the above". That covers narration, character voices, and reading chats aloud.
> - Offered three ways to build it, the owner answered "A". A means voice cloning from the person's own MP3 or WAV now, through the engine the app already ships, with in-app training in the next update.

# harnessUI 0.2.0 Voice: local voice models, cloning from your own audio, and reading aloud

This is the spec for the Voice part of release 0.2.0. It ships in the same version and pull request as the Studio work in `docs/dev/0.2.0/design.md`, and it is built on top of that branch. Its plan will be `docs/dev/0.2.0/voice-plan.md`.

The research behind it was done on 2026-11-26. Facts marked *(measured)* were run in the build container: 4 CPU cores, no graphics card, the pinned engine's Linux release. The engine facts come from the pinned llama.cpp `b12110`: its `tools/tts/README.md`, its `tools/server/README.md`, and the strings in the release's own libraries.

## 1. Why

The owner wants voices to work like Studio's pictures and videos:
- local models from Hugging Face or the person's own disk, that just work;
- an ElevenLabs-style way to make a voice from their own MP3 or WAV files.

They want to use the voices three ways: narration, character voices, and having chats read aloud.

The engine the app already ships can do this. The pinned `llama-server` (llama.cpp `b12110`) has a `POST /tts` endpoint. It speaks text with a voice model and clones a voice from a reference audio file sent with the request. So Voice needs no new engine program, no Python, and nothing in the cloud.

Training a voice's weights inside the app is not possible without Python today. llama.cpp's trainer is experimental and has no speech data path, and every mainstream voice trainer is PyTorch. So it moves to the next update (section 4). Cloning from a short clip, which is what ElevenLabs' instant cloning is, comes now.

## 2. Goals

1. **A Voice section.** It looks and works like Studio:
   - a text box;
   - a model chip;
   - a voice picker;
   - a Create button;
   - a list of the clips made, each with a player.

   Its *Get models* view works like Studio's: search Hugging Face, a verdict on each model ("Works in Voice", "Clones voices" or "Fixed voices"), a detail panel, and one Download for the model and its paired file.
2. **Make a voice from your own audio.** It works like ElevenLabs' instant cloning:
   - add one or more MP3, WAV, FLAC or OGG files;
   - the app decodes them, finds the clearest speech, and shows it on a waveform that the person can adjust;
   - the person names the voice and confirms they have the right to clone it;
   - they hear a sample at once;
   - the voice is saved and can be used with any voice model that clones.
3. **Bring your own.**
   - Any voice model the pinned engine can speak with works, whether it comes from Hugging Face or is already on disk. That includes models someone has already trained on a voice elsewhere and converted to GGUF.
   - Models with built-in voices (Soprano, LFM3-Audio, Pocket TTS's no-cloning weights, Chatterbox's default voice) show those voices in the picker.
4. **Read aloud in chat.**
   - Every finished assistant reply gets a speaker button.
   - Settings has *Read replies aloud* (off by default). When it is on, each new reply is read as its sentences arrive.
5. **Narration and characters.**
   - Long text is split into sentences, spoken in order, and joined into one file, with progress and Cancel.
   - A line that starts with a saved voice's name and a colon ("Mira: Where were you?") is spoken in that voice. Every other line is spoken in the selected voice.
6. **Everything stays on the computer.** The audio a person adds, the voices made from it, and every clip stay in the app's folders. Nothing is uploaded.

## 3. Non-goals and anti-goals

**Non-goals for 0.2.0:**
- **Training or fine-tuning in the app.** That is the next update (section 4).
- **Recording from the microphone.** Files only, as the owner described ("from users mp3 or wav"). Recording would also need microphone permission on each platform.
- **Cloud voice services.** No ElevenLabs, OpenAI or other API.
- **Speech-to-text and voice chat.** The pinned server has `/v1/audio/transcriptions` and `/v1/realtime`, but they are not part of this release.
- **MP3 export.** WAV only. MP3 encoders are LGPL or patent-encumbered, and WAV plays everywhere.
- **Chatterbox Multilingual.** Its classifier-free guidance is not supported by `llama-server` at `b12110`. It shows as "Can't run in Voice" with that reason.

**Anti-goals.** Any of these would make Voice a failure even if it technically works:
- **A clone without the person confirming they have the right to make it.**
- **A voice, clip or added audio file leaving the computer,** or appearing in a log with its path or content.
- **A dead end.** For example, a Voice section that opens with nothing to do, or a read-aloud button that silently does nothing. Every empty state says what to do and offers it.
- **Making the chat model slower or unloading it without saying so.** Reading a reply aloud must not take away the model the person is chatting with.
- **Weakening Studio's protections.** That means the child-safety prompt check, pickle refusal, SHA-256 checks and the spawn helper. Voice adds none of the risky inputs those guard against, and must not route around them.

## 4. Approaches (the owner picked A)

| | What | Trade-off |
|---|---|---|
| **A (picked)** | Cloning now, through the `llama-server` the app already ships (`POST /tts` with a reference clip). In-app training comes in the next update. | Ships in 0.2.0 with no new runtime. A clone made from a short clip is close to, but not as close as, a voice trained on 30 minutes of audio. |
| B | Training in 0.2.0, through a downloadable training pack (a pinned Python and PyTorch environment), even if it delays the release. | The closest result, but GBs to download, a GPU in practice, and a large new attack surface. It would delay everything. |
| C | Training on a cloud GPU, with the trained weights downloaded back. | Needs an account, payment and upload of the person's voice, which runs against goal 6. |

For the next update, A leaves room for training. A saved voice records its kind (`reference` now; `weights` later), and the engine already loads LoRA adapters (`--lora`). Whether its speech path applies them is unverified at `b12110`, and the training design will settle it.

## 5. What the pinned engine provides (verified)

**The endpoint.** `llama-server` `b12110` has `POST /tts`, which takes:
- `input`, the text;
- `lang`;
- `voice`, one of the model's built-in voices from `GET /props` → `voices`;
- `speaker_ref`, a WAV or MP3 reference, uploaded as multipart or as `speaker_ref_b64`;
- `seed`;
- `n_predict`, in audio frames, default 512;
- `repeat_penalty`, default 1.05;
- `top_k` and `top_p`;
- `response_format`, either `wav` or `pcm` (float32 little-endian mono, with the rate in `Content-Type`);
- `stream`.

A predefined voice and `speaker_ref` can't be combined. `GET /props` → `modalities.audio` says whether the loaded model can speak.

**Supported families.** The model families at `b12110` are these (mmproj projector types in brackets):

| Family | Clones | Built-in voices | Languages | Ready GGUF |
|---|---|---|---|---|
| Qwen3-TTS 12 Hz 1.7B Base (`qwen3tts_gen`, `qwen3tts_spkenc`) | yes; works poorly without a reference | none | zh en de it pt es ja ko fr ru | `ggml-org/Qwen3-TTS-12Hz-1.7B-Base-GGUF`, Apache-2.0 |
| Chatterbox Turbo (`chatterbox`, `chatterbox_spkenc`) | yes (optional) | `default` | English | `ggml-org/chatterbox-turbo-GGUF`, MIT (upstream retired August 2026) |
| Soprano 1.1 80M (`soprano`) | no | one fixed voice | English | `ggml-org/Soprano-1.1-80M-GGUF`, Apache-2.0 |
| Pocket TTS (`pockettts_gen`, `pockettts_spkenc`) | cloning weights only; no-cloning weights have built-in voices | yes, in no-cloning weights | set by the weights | community GGUFs only (for example `Serveurperso/pocket-tts-GGUF`, CC-BY-4.0) |
| LFM3-Audio (`lfm3a_gen`) | no | `female`, `male` | 19 | `LiquidAI/LFM3-Audio-2B-GGUF`, licence "lfm1.0" (custom) |
| Chatterbox Multilingual | needs CFG, not on the server | n/a | 23 | `ggml-org/chatterbox-GGUF`, MIT. "Can't run in Voice" |

**How a voice model is recognised** (read from the files):
- A voice model is a backbone GGUF plus an mmproj GGUF.
- The mmproj has `general.type = mmproj` and `clip.has_gen_audio_encoder = true`, and `clip.gen.audio.projector_type` names the family.
- It **clones** when `clip.has_audio_encoder = true` and `clip.audio.projector_type` ends in `_spkenc`.
- The backbone's architecture doesn't identify a voice model: it is `qwen3tts`, `qwen3` (Soprano) or `gpt2` (Chatterbox). The projector is what matters.

**Ready-file sizes:**

| Model | Backbone | mmproj | Total |
|---|---|---|---|
| Qwen3-TTS | Q4_K_M 1,035,965,280 bytes | Q8_0 446,422,912 bytes | about 1.5 GB |
| Chatterbox Turbo | Q8_0 481,791,712 bytes | F16 555,371,392 bytes | about 1.0 GB |
| Soprano | Q8_0 84,921,216 bytes | F16 60,821,216 bytes | about 146 MB |

**Speed (measured,** 4 CPU cores, no GPU):

| Model | Speed (× real time) | Detail | Ready after start |
|---|---|---|---|
| Soprano | 13.6× | | |
| Chatterbox Turbo, cloning | 0.7× | 13.4 s for 6.7 s of speech through `/tts` | 3 s |
| Qwen3-TTS Q4, cloning | 0.45× | 16.2 s for 7.0 s | 7 s |

- The reference clip is encoded again on every request.
- Streaming gave the first audio at 12.4 s, against 16.2 s for the whole clip, for Qwen3-TTS. For Chatterbox it gave nothing until the end.
- So on a CPU, cloned speech takes several seconds per sentence before it starts. With a GPU it should be much faster. That is unmeasured, and the builder measures it on CI's macOS runner.
- Output is 24 kHz for Qwen3-TTS and Chatterbox, and 32 kHz for Soprano.

## 6. The design

### 6.1 Where Voice lives

Voice is a new top-level section, beside Chat and Studio, with its own sidebar button and shortcut. It reuses Studio's look and components: the composer, the model chip, the model sheet, *Get models* with its verdicts and detail panel, and the side panel for settings. A person who knows Studio already knows Voice.

*(My call. The alternative is a third kind inside Studio, beside Picture and Video. It would share the gallery grid but crowd Studio's composer with voice-only controls. Voice also serves chat, through read aloud, which a Studio tab would hide.)*

**Layout.** It mirrors Studio:
- **Top:** the voice picker and the model chip.
- **Middle:** the clips, newest first. Each card shows the text's first line, the voice, the length, and a player with play/pause, a seek bar and a 0.75×–1.5× speed control. It also has *Save as…*, *Copy text*, *Make again* and *Delete*.
- **Bottom:** the text box, with a character count, the Create button, and a hint about "Name:" lines once two or more voices exist.
- **Empty state, no model:** "Get a voice model" with the three suggested models (6.3) and their sizes.
- **Empty state, a model but no voice of your own:** for a cloning model, *Make a voice from your audio* beside the model's built-in voices, if any.

### 6.2 The voice engine

**The process.** Voice runs its own `llama-server`, a second managed child separate from the chat model's:
- it is started through `runtime::llama::server::start`, so the spawn helper, loopback port, per-launch API key, health wait, process group or Job Object, and stderr tail all apply unchanged;
- its arguments are the backbone (`-m`) and the mmproj (`--mmproj`), plus whatever the engine needs for speech (the builder reads `build_args`; vision-only behaviour such as the image-file retry does not apply);
- it is not part of `RuntimeManager`'s single chat slot.

**Lifetime.**
- It starts on first use: Create, a read-aloud press, or a sample.
- It stays loaded while used, and stops after 5 minutes idle. The time is a Settings value; 0 means "keep loaded".
- It stops at quit and for an update install, as Studio's local jobs do since G3.
- If it crashes it is restarted once. Repeated failures show the engine's own last error lines, as the chat model's failures do.

**Memory.** Read aloud must never unload the chat model:
- **Placement.** Before it starts, the voice engine is placed with the same fit estimate the chat model uses. It goes on the GPU if it fits beside what is already loaded there, and otherwise on the CPU.
- **The CPU note.** When the CPU was chosen because the chat model fills the GPU, the Voice section and the read-aloud control show one line: "Voice runs on the processor because the chat model fills the graphics memory, so it is slower."
- **Studio.** A Studio local job takes the GPU lease as it does today, and the lease also stops the voice engine. Voice requests made meanwhile wait, with "Waiting for Studio to finish…" and a Cancel. When the job ends, the voice engine starts again on next use. It is not reloaded at once.
- **Chat loads.** A chat model load never waits for the voice engine. If the chat model needs memory the voice engine holds, the voice engine is stopped first.

### 6.3 Models: getting them, bringing them, and the verdict

**Suggested models.** These go in the signed catalog, as a new `voices` section in `catalog/models.json`. Older builds ignore it, and entries that don't validate are hidden with a reason.

| Model | Backbone | mmproj | Why |
|---|---|---|---|
| Qwen3-TTS 1.7B Base | Q4_K_M | Q8_0 | best cloning, 10 languages |
| Chatterbox Turbo | Q8_0 | F16 | faster, English, clones |
| Soprano 1.1 80M | Q8_0 | F16 | tiny, fast even on a CPU, one fixed voice: good for reading aloud on a computer without a GPU |

Each entry pins the repo, revision, file names, sizes and SHA-256. That data is in section 5 and the Hugging Face API.

**Search and detail.** *Get models* searches Hugging Face for GGUF repos and reads the mmproj headers the way Studio reads safetensors headers:
- with ranged reads;
- with Studio's caps and parallelism;
- reading only a quantisation group's smallest file.

It pairs one backbone with one mmproj per repo. The default quantisation is the smallest the fit estimate calls comfortable, and it can be changed in the detail panel, as in Studio.

**Verdicts:**
- **"Works in Voice · Clones voices"**, **"Works in Voice · Fixed voices"**, or **"Works in Voice · Built-in voices: …"** when the names are in the file's metadata. Otherwise the voices are read from `/props` after the first load.
- **"Can't run in Voice"**, with the reason. The reasons are:
  - no speech projector in any mmproj;
  - a projector type the pinned engine doesn't know (*"made for a newer engine"*);
  - Chatterbox Multilingual (*"needs a feature the engine's server doesn't have yet"*);
  - no backbone with the mmproj.

**Bring your own.** *Add a model file* takes a backbone and an mmproj from disk:
- either picked together, or the mmproj found beside the backbone, as `hub::image_file` does for vision projectors today;
- with the same verdict;
- files outside the models folder are copied into `@imported/<sha16>/`, as in Studio, and *Use it where it is* is offered.

A fine-tuned or trained voice model converted to GGUF is just another model here.

**Downloads.** Downloads use `DownloadManager` groups, with the backbone and the mmproj in one group. Each file is checked against its size and SHA-256, exactly as chat and Studio downloads are. Pause, Cancel and *Use it* work as in Studio. Licences are shown before download with Studio's licence rows. Pocket TTS's CC-BY-4.0 attribution and its card's consent clause are shown as its notice.

**Registration.** A downloaded or added voice model is checked before it joins the installed list: both headers are read, and the verdict must be "Works". It is stored in its own table. Studio's `gen_models` kinds are picture and video only and stay that way.

### 6.4 Making a voice from your own audio

The flow is *Make a voice*, from the voice picker or the empty state. It is a dialog with three steps.

**1. Add audio.**
- Drop or pick one or more MP3, WAV, FLAC or OGG Vorbis files.
- Limits: at least 10 seconds of speech in total, at most 30 minutes, and at most 500 MB per file. The dialog recommends 1 to 2 minutes of clear speech by one person, with no music.
- Files are decoded natively with Symphonia (MPL-2.0, already allowed by `deny.toml`). They are resampled to 24 kHz mono with rubato.
- A file that can't be decoded, or holds less than one second of sound, is named with the reason. It doesn't stop the others.

**2. Choose the clip.**
- The app finds the clearest speech. It scores windows by speech energy, silence and clipping, and builds a reference of up to 20 seconds from the best stretches, joined with 0.2-second gaps.
- It shows them on one waveform per file, with the chosen parts highlighted.
- The person can drag a highlight, add one, or remove one. *Play the reference* plays exactly what the engine will hear.
- The reference is peak-normalised and trimmed of leading and trailing silence.

*(My call on 20 seconds. The engine encodes a speaker embedding, so beyond about 20 seconds more audio adds time on every request, as the measurements show, without a clearer voice. The builder tunes the scoring by ear.)*

**3. Name and confirm.**
- **The name.** It is used in "Name:" lines, so it must be unique among the person's voices (case-insensitive), 1 to 40 characters, and can't contain a colon.
- **A language,** for models that take one.
- **A required tick:** "This is my voice, or I have the speaker's permission to clone it. I won't use it to pretend to be someone or to deceive people." The dialog also says plainly that cloning a public figure or anyone without their consent is not allowed.
- **Save** is disabled until the tick.
- **On Save,** the app writes the reference WAV and a record, then speaks a fixed sample sentence with the installed cloning model. If none is installed, it offers the suggested ones.

**What is stored** for each voice:
- its id, name and language;
- its kind (`reference`);
- the reference WAV's path and SHA-256;
- the source files' SHA-256 and durations, never their paths;
- the consent statement's text, its version and the time it was given;
- the created time.

The files go under `<home>/harnessUI/voice/voices/<id>/`. Deleting a voice deletes that folder and its row. Clips already made with it stay, labelled "voice deleted".

**Built-in voices.** A model's built-in voices appear in the picker under the model's name and need no consent. Built-in voices and the person's own voices are kept apart in the picker:
- with a cloning model, the person's voices come first, then any built-in voices;
- with a fixed-voice model, only its built-in voices appear. The person's voices are listed greyed, with "This model can't use your voices; pick a model that clones."

### 6.5 Making clips: narration and characters

**Splitting text.** Text is split at sentence ends (`. ! ? …` and line breaks), keeping each piece under about 300 characters. A longer sentence is split at a comma or a space. Each piece is one `/tts` request.

**Requests.**
- `seed` is fixed per clip, so every piece in a voice sounds the same.
- `n_predict` is set from the family's frame rate so that a piece can never be cut short. For example, 12 Hz × 40 s for Qwen3-TTS.
- The pieces are joined with 0.25 seconds of silence after a sentence and 0.6 seconds after a paragraph.

**Progress.** The card shows "Speaking 3 of 12…" with Cancel, and the first piece can be played as soon as it is ready. Cancel keeps the pieces already made as a shorter clip, labelled "stopped".

**Character lines.** A line matching `^<voice name>:` (case-insensitive, after trimming) is spoken in that voice, without the name.
- A name that matches no voice is spoken in the selected voice as written, name included, and the card notes "No voice called 'Bob'".
- Character voices need a cloning model.
- With a fixed-voice model, "Name:" lines are read as plain text, and the hint says why.

**The output.**
- One WAV per clip, at the model's sample rate, 16-bit PCM.
- A `LIST/INFO` chunk carries `ISFT` = "harnessUI" and `ICMT` = "Made with an AI voice". This is the honest label on every file, and it survives *Save as…*.
- Clips are stored under `<home>/harnessUI/voice/clips/YYYY-MM/`, with a row holding the text, voice ids, model, seed, length and state.
- They are played in the webview as blob URLs, fetched by clip id over IPC, as `useOutputUrl` does for Studio. The CSP already allows `media-src blob:`. The webview never sends a path.

### 6.6 Read aloud in chat

**The button.**
- A speaker button sits after Copy on every finished assistant reply.
- Pressing it reads the reply with the default voice and model from Settings → Voice.
- Pressing it again, or pressing another reply's button, stops the first.
- Only one reply reads at a time.

**What is read.**
- A plain-text form of the reply's markdown.
- Code blocks are replaced by "Code block." Tables are replaced by "Table."
- Links are read by their text, and markdown symbols are dropped.
- Reasoning and tool output are not read.

**Read replies aloud.** When this setting is on, a streaming reply is read sentence by sentence as complete sentences arrive. Starting a new message, pressing Stop, or switching chats stops reading.

**States.**
- Loading the voice engine shows a spinner on the button.
- Waiting for Studio shows "Waiting for Studio…".
- With no voice model, the button opens Voice → *Get models*, with one line saying why.
- An engine failure shows the error under the reply. It never fails silently.

**Storage.** Read-aloud audio is played and thrown away, not saved as clips. *Save as clip* on the playing reply keeps it.

### 6.7 Settings → Voice

A new Settings page, inserted after Studio. It is also added to e2e `SETTINGS_PAGES` in the same order. It holds:
- the default voice model;
- the default voice;
- *Read replies aloud* (off);
- the read-aloud speed (0.75×–1.5×, playback rate only);
- *Keep the voice engine loaded* for 5 minutes (0 to 60, 0 meaning "until quit");
- *Run voice on* Automatic (6.2) or Processor only;
- the folder Voice keeps clips and voices in, defaulting to `<home>/harnessUI/voice`.

Voice prefs are one JSON setting with a version, like Studio's prefs. The first version is 1 and needs no migration.

### 6.8 Storage and the database

**The migration.** One new migration, `V5`, appended per `crates/store/src/schema.rs`'s rule, with `AUTOINCREMENT` where ids must not be reused. It adds three tables:
- `voice_models`: the files, SHA-256s, family, `can_clone`, built-in voices, licence, `local` or `downloaded`, and `@imported` as in Studio;
- `voices`: 6.4's record;
- `voice_clips`: 6.5's record.

A 0.1.2 or earlier 0.2.0 database upgrades by that one step and nothing else changes.

**Files.** Everything lives under the Voice folder: voices, clips, and a `.partial/` folder for clips being made. Deleting follows Studio's rule: the app deletes only what it made, never a person's own files.

### 6.9 Safety and privacy

**Consent.**
- The consent gate (6.4) is required for every voice made from audio.
- The text and time of the consent are kept with the voice.
- There is no identity check. Like ElevenLabs' instant cloning, it is an attestation.

**Labelling.** Every saved WAV carries the AI label (6.5).

**Privacy.**
- No added audio, voice or clip leaves the computer.
- Logs name voices by id. They never record paths, file names or text.
- The voice engine runs on loopback with its own API key, like the chat engine.

**The text checks.**
- Studio's child-safety prompt check stays exactly as it is for picture and video prompts.
- Voice text is not put through it. That check judges image prompts, and speech of text the person typed or a chat reply is not the harm it guards against.

*(My call; it costs nothing to revisit.)*

**Licences.** A model's licence and notice are shown before download, as in Studio.

**Licence files.** Symphonia (MPL-2.0) and rubato (MIT or Apache-2.0) join `THIRD-PARTY-NOTICES.md` through `scripts/notices.py`.

### 6.10 Docs and tests

**Docs.**
- `ARCHITECTURE.md` gets a Voice section: the engine, memory, storage, verdicts and read aloud.
- `SECURITY.md` gets consent, labelling, what stays local, and the voice engine's loopback and key.
- `README.md` gets a short Voice paragraph.
- `CHANGELOG.md`'s 0.2.0 entry gets the Voice lines, in user words.
- `docs_test.py` pins:
  - the consent statement;
  - "nothing leaves the computer";
  - the AI label.

**Tests.**
- **A contract test** against the real pinned `llama-server` and the 146 MB Soprano files. It checks that `/props` reports audio and that `/tts` returns a WAV of plausible length. It runs where the chat contract tests run.
- **Cloning.** Same-shape tests with a cloning model run in the nightly or manual workflow, not on every push, because of the 1 GB download.
- **Unit tests:**
  - the header verdicts (each family, unknown projector, Multilingual, a missing pair);
  - text splitting and "Name:" lines;
  - the markdown-to-speech text;
  - the reference picker on synthetic audio;
  - the WAV joiner and its `LIST/INFO` chunk;
  - the migration from a 0.1.2 database.
- **e2e:** m8 gains the Voice section and the Settings page. A fake `/tts` server (like `e2e/fake_sd`) lets an e2e script make a voice from a bundled short WAV, create a two-line character clip, and read a chat reply aloud.

## 7. Left to the builder

- How the second `llama-server` is held: a small voice manager beside `RuntimeManager`, or a generalised manager. That includes how the fit estimate is extended to two models.
- Any speech-specific `llama-server` arguments, read from `build_args` and the engine. Also which existing fixed arguments don't apply.
- The reference-scoring method and its thresholds, tuned by ear on real recordings.
- The exact Hugging Face search terms for voice models, and how repos are grouped and paired.
- Component structure in the webview, following Studio's patterns and `src/lib/words.ts`.
- The waveform drawing, which needs no new library if a canvas does it.

## 8. Calls I made (for the owner to check)

1. Voice is its own section beside Chat and Studio, not a tab inside Studio (6.1).
2. Files only, no microphone recording, for 0.2.0 (3).
3. The reference is up to 20 seconds, picked automatically and adjustable (6.4).
4. The three suggested models are Qwen3-TTS 1.7B, Chatterbox Turbo and Soprano (6.3).
5. Read aloud never unloads the chat model. Voice runs on the processor when the graphics memory is full, and says so (6.2).
6. *Read replies aloud* is off by default (6.6).
7. Voice text is not put through Studio's picture prompt check (6.9).
8. WAV only, with an "AI voice" label inside every file (6.5).
