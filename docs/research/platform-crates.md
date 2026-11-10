# platform-crates — research

## Summary

All versions below were read from the crates.io API on 2026-11-10 (version + publish date of the max stable release) and cross-checked against upstream source files; items I could not verify first-hand are marked medium/low.

Headline decisions for ARCHITECTURE.md:
(1) Storage: rusqlite with `bundled` (FTS5 incl. the trigram tokenizer is compiled in; no feature flag needed). Newest is 0.41.1, but rusqlite_migration 2.6.0, tokio-rusqlite 0.8.0, deadpool-sqlite 0.14.0 and refinery 0.10.0 all still cap at rusqlite 0.40, so pin 0.40.2 today. Do not mix with sqlx or tauri-plugin-sql (libsqlite3-sys `links` conflict). Run SQLite on a dedicated thread, WAL mode.
(2) Secrets: keyring 4.2.0 is now a thin facade (`v1` feature) over keyring-core 1.0.1 + per-platform store crates; the v3 feature names (apple-native, windows-native, sync-secret-service) are gone. Windows Credential Manager hard limit is 2560 bytes per blob (1280 UTF-16 units through set_password), so one entry per secret or a keychain-held master key. Unsigned/ad-hoc macOS builds re-prompt per item after every rebuild.
(3) Process control: process-wrap 10.0.1 (successor of command-group) gives JobObject on Windows and ProcessGroup on macOS, but only sets JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE when KillOnDrop is also wrapped. tauri-plugin-shell 2.6.0's job object does NOT set kill-on-close, so it does not survive an app crash. macOS has no kernel equivalent: use a small reaper helper (pipe-EOF and/or kqueue EVFILT_PROC/NOTE_EXIT) plus stale-PID cleanup at launch.
(4) Terminal: plain piped tokio::process for one-shot commands; portable-pty 0.9.0 (last release 2025-02-11) only if interactive TTY is needed later. powershell.exe 5.1 is the only shell guaranteed on Windows (no `&&`/`||`, exit code collapsed to 0/1, non-UTF-8 output by default); zsh -lc on macOS (does not read ~/.zshrc).
(5) Destructive-command policy: follow Codex CLI (tree-sitter-bash + tree-sitter-powershell, allowlist of plain word-only command sequences, everything else prompts). Codex explicitly treats heuristics as secondary to an OS sandbox; without a sandbox, prompt for everything not proven read-only.
(6) Git: git2 0.21.0 with `vendored-libgit2` + `https` (network features are no longer default). On Windows/macOS HTTPS uses WinHTTP/SecureTransport, so no OpenSSL. gix 0.90.0 still has no push. Fallback: detected system git CLI.
(7) Documents/misc: pdf-extract 0.12.1 (pure Rust, panics possible) or pdfium-render 0.9.5 (needs bundled PDFium); chardetng 1.0.0 + encoding_rs 0.8.44; jsonschema 0.61.0 with default features off; zip 9.5.0; serde-saphyr 2.1.1 for YAML (serde_yml is RUSTSEC-flagged and deprecated).

## Findings

### rusqlite 0.41.1 (2026-10-28, MSRV 1.89, MIT) with feature `bundled` compiles SQLite 3.54.1 with FTS5 enabled; the trigram tokenizer is available. No extra cargo feature is needed for FTS5.

Confidence: high

libsqlite3-sys 0.39.1 (2026-10-25) tarball: sqlite3.h has `#define SQLITE_VERSION "3.54.1"`; build.rs passes -DSQLITE_ENABLE_FTS5, -DSQLITE_ENABLE_FTS3, -DSQLITE_ENABLE_JSON1, -DSQLITE_ENABLE_RTREE, -DSQLITE_ENABLE_STAT4, -DSQLITE_ENABLE_LOAD_EXTENSION=1, -DSQLITE_THREADSAFE=1, -DSQLITE_USE_URI and -DSQLITE_DEFAULT_FOREIGN_KEYS=1 (foreign keys are ON by default in the bundled build). sqlite3.c contains the "trigram" tokenizer. rusqlite 0.40.2 (2026-08-08) uses libsqlite3-sys ^0.38.1; upgrade.sh at tag v0.40.2 names sqlite-amalgamation-3530200 (SQLite 3.53.2). Windows has no system SQLite, so `bundled` is mandatory there anyway.

Source: https://crates.io/api/v1/crates/rusqlite ; https://static.crates.io/crates/libsqlite3-sys/libsqlite3-sys-0.39.1.crate (build.rs, sqlite3/sqlite3.h) ; https://raw.githubusercontent.com/rusqlite/rusqlite/v0.41.1/Cargo.toml

### The rusqlite helper ecosystem lags the 0.41 release: pin rusqlite 0.40.x if you want rusqlite_migration or tokio-rusqlite today.

Confidence: high

Dependency requirements on crates.io: rusqlite_migration 2.6.0 (2026-05-28, MSRV 1.95, Apache-2.0) -> rusqlite ^0.40.0; tokio-rusqlite 0.8.0 (2026-09-06) -> ^0.40.1; deadpool-sqlite 0.14.0 (2026-08-26) -> ^0.40.0; refinery-core 0.10.0 (2026-10-02) -> >=0.23, <=0.40; r2d2_sqlite 0.36.0 (2026-11-09) -> ^0.41 (r2d2 itself 0.8.10, last release 2022-06-21); async-sqlite 0.7.0 (2026-11-01) -> ^0.41.0. rusqlite_migration tracks schema version in PRAGMA user_version (no bookkeeping table), API: `const MIGRATIONS: Migrations = Migrations::from_slice(&[M::up("...")]); MIGRATIONS.to_latest(&mut conn)`, plus `MIGRATIONS.validate()` for a unit test; README recommends setting journal_mode outside migrations. refinery is heavier (regex, time, url, walkdir, own history table) and gives nothing extra for a single embedded DB.

Source: https://crates.io/api/v1/crates/{rusqlite_migration,tokio-rusqlite,deadpool-sqlite,refinery-core,r2d2_sqlite,async-sqlite}/<ver>/dependencies ; https://raw.githubusercontent.com/cljoly/rusqlite_migration/master/README.md

### rusqlite and sqlx (and tauri-plugin-sql) cannot share one binary at current versions; choose one. rusqlite is the better fit here.

Confidence: high

libsqlite3-sys declares `links = "sqlite3"`, so Cargo allows exactly one version in the graph. sqlx-sqlite 0.9.0 (sqlx 0.9.0, 2026-05-21, MSRV 1.94) requires libsqlite3-sys >=0.30.1, <0.38.0; rusqlite 0.40 needs ^0.38.1 and 0.41 needs ^0.39.0, so sqlx 0.9 only coexists with rusqlite <=0.39.0. tauri-plugin-sql 2.6.0 depends on sqlx ^0.8 (sqlx-sqlite 0.8.6 -> libsqlite3-sys ^0.30.1), which conflicts with every recent rusqlite. sqlx is viable (Codex CLI uses sqlx =0.9.0 for SQLite) but brings an older bundled SQLite, async-only API and compile-time-macro/offline-data workflow; rusqlite gives direct synchronous access to FTS5, backup, hooks and pragmas.

Source: https://crates.io/api/v1/crates/sqlx-sqlite/0.9.0/dependencies ; https://crates.io/api/v1/crates/tauri-plugin-sql ; https://raw.githubusercontent.com/rusqlite/rusqlite/v0.41.1/libsqlite3-sys/Cargo.toml ; https://raw.githubusercontent.com/openai/codex/main/codex-rs/Cargo.toml

### Recommended async pattern: one dedicated SQLite thread (writer) addressed through a channel, optionally one extra read-only connection; never call rusqlite directly inside an async Tauri command.

Confidence: medium

tokio-rusqlite 0.8.0 is exactly this pattern (`Connection::open(path).await`, `conn.call(|conn| { ... }).await`, `conn.close().await`; deps are only crossbeam-channel + tokio sync, `#![forbid(unsafe_code)]`). A hand-rolled std::thread + mpsc + oneshot actor is about the same amount of code and removes the version-lag problem. spawn_blocking + Mutex<Connection> works but serialises on a mutex and scatters blocking calls; r2d2/deadpool pools add little for a single-user desktop DB where SQLite allows one writer at a time. rusqlite::Connection is Send but not Sync (from memory, not re-verified today).

Source: https://raw.githubusercontent.com/programatik29/tokio-rusqlite/master/README.md ; https://crates.io/api/v1/crates/tokio-rusqlite/0.8.0/dependencies

### FTS5 trigram tokenizer facts for chat search: substring matching, but queries shorter than 3 characters match nothing; LIKE/GLOB become index-assisted.

Confidence: high

sqlite.org/fts5.html: `CREATE VIRTUAL TABLE tri USING fts5(a, tokenize="trigram")`; options `case_sensitive` (0 default) and `remove_diacritics` (0 default; may only be 1 when case_sensitive is 0). 'Substrings consisting of fewer than 3 unicode characters do not match any rows when used with a full-text query.' Unless remove_diacritics is set, trigram tables also support indexed LIKE and GLOB; a LIKE with an ESCAPE clause cannot use the index; case_sensitive 1 indexes GLOB only. With detail=none/column, full-text queries may not contain tokens longer than 3 characters. FTS5 tokens are capped at 32768 bytes. Practical design: external-content FTS5 table over messages kept in sync by triggers, trigram tokenizer for substring/CJK search, fall back to a plain LIKE scan for 1-2 character queries. WAL setup is `conn.pragma_update_and_check(None, "journal_mode", &"WAL", |_| Ok(()))` as shown in the rusqlite_migration README.

Source: https://www.sqlite.org/fts5.html (section 'The Trigram Tokenizer') ; https://raw.githubusercontent.com/cljoly/rusqlite_migration/master/README.md

### keyring 4.x was restructured: keyring 4.2.0 (2026-08-29, MSRV 1.88) has only two features, `v1` (default) and `cli`; the API moved to keyring-core and each backend is its own crate.

Confidence: high

keyring 3.6.3 (2025-07-27) was the last v3, with features apple-native, windows-native, sync-secret-service, async-secret-service, linux-native, crypto-rust, crypto-openssl, vendored, tokio, async-io. v4 `v1` = apple-native-keyring-store/keychain + windows-native-keyring-store + zbus-secret-service-keyring-store and exposes the old `keyring::Entry::new(service, user)` with set_password / get_password / set_secret / get_secret / delete_credential, plus `Entry::store_status()`. Upstream says apps wanting control should depend directly on keyring-core 1.0.1 (2026-10-13) and the store crates: apple-native-keyring-store 1.0.2 (2026-08-06; features `keychain`, `protected`, no defaults) and windows-native-keyring-store 1.1.0 (2026-05-24; default feature `search` pulls in regex, >1 MB, can be disabled). Setup: `keyring_core::set_default_store(windows_native_keyring_store::Store::new()?)` / `apple_native_keyring_store::keychain::Store::new()?`. Errors include NoEntry, TooLong(String, u32), NoStorageAccess, NoDefaultStore. Use the `keychain` module on macOS: `protected` needs a provisioning-profile-signed app and otherwise fails with PlatformError -34018.

Source: https://raw.githubusercontent.com/open-source-cooperative/keyring-rs/main/Cargo.toml ; .../keyring-rs/main/src/v1.rs ; https://raw.githubusercontent.com/open-source-cooperative/apple-native-keyring-store/main/README.md ; https://raw.githubusercontent.com/open-source-cooperative/keyring-core/main/src/lib.rs ; https://crates.io/api/v1/crates/keyring

### HARD LIMIT confirmed: a Windows Credential Manager blob is at most 2560 bytes (CRED_MAX_CREDENTIAL_BLOB_SIZE = 5*512); via keyring's set_password that is 1280 UTF-16 code units.

Confidence: high

Microsoft CREDENTIALW docs: 'CredentialBlobSize ... cannot be larger than CRED_MAX_CREDENTIAL_BLOB_SIZE (5*512) bytes'; generic TargetName max 32767 chars; UserName max 513; Comment/TargetAlias max 256; max 64 attributes. windows-native-keyring-store encodes set_password as UTF-16LE and returns Error::TooLong when the blob exceeds the limit; set_secret(&[u8]) stores raw bytes (2560 bytes of UTF-8). Consequences: never store all MCP env vars as one JSON blob; use one entry per secret (target name defaults to `<user>.<service>`), and long values (large JWTs, service-account JSON) will not fit at all. Other documented behaviour: default persistence is Enterprise (roams with roaming profiles; a `persistence` modifier accepts Session/Local/Enterprise), and the README warns that Credential Manager loses updates when credentials are written/deleted concurrently, so writes should be serialised (the crate uses a per-user named mutex).

Source: https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw ; https://raw.githubusercontent.com/open-source-cooperative/windows-native-keyring-store/main/src/utils.rs ; .../windows-native-keyring-store/main/README.md ; .../src/lib.rs

### macOS file-based keychain access is tracked by the creating app's code-signing designated requirement, so unsigned/ad-hoc builds are re-prompted after each rebuild; a stable signing identity (even self-signed) fixes it.

Confidence: medium

Apple TN2206: keychain access controls give 'Free access to the keychain item by the creating application and tracked with its DR'; stability 'is determined through the designated requirement (DR) mechanism, and does not depend on the nature of the certificate authority used... Self-signed identities and homemade certificate authorities (CA) work by default for this case.' Apple TN3137: the file-based keychain uses access control lists, while the data protection keychain uses access groups from entitlements that 'must be authorized by a provisioning profile'. That an ad-hoc signature's designated requirement is the cdhash (changing on every build) comes from secondary reports, not an Apple page I could load. Each keychain item has its own ACL, so N per-secret items can mean N password prompts after a rebuild. Mitigations: Developer ID signing for releases, one stable self-signed identity for dev builds, a non-keychain dev store, and fewer items (e.g. a single master key).

Source: https://developer.apple.com/library/archive/technotes/tn2206/_index.html ; https://developer.apple.com/tutorials/data/documentation/technotes/tn3137-on-mac-keychains.json ; secondary: https://github.com/PsychQuant/logos/issues/101

### Windows: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE is the mechanism that kills children when the app crashes or is force-quit; process-wrap only sets it when KillOnDrop is also wrapped, and tauri-plugin-shell never sets it.

Confidence: high

Microsoft: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE (0x00002000) 'Causes all processes associated with the job to terminate when the last handle to the job is closed' and requires JOBOBJECT_EXTENDED_LIMIT_INFORMATION; jobs can be nested. The OS closes the app's handles on any termination, so the job must be held only by the app. process-wrap 10.0.1 (2026-09-23, MSRV 1.87, Apache-2.0 OR MIT; 'Successor to command-group', whose last release is 5.0.1 from 2023-11-18): features std / tokio1, wrappers JobObject, CreationFlags, KillOnDrop, ProcessGroup, ProcessSession; src/windows.rs make_job_object() sets the flag only `if kill_on_drop`, and the tokio JobObject wrapper computes that from `core.has_wrap::<KillOnDrop>()`. It spawns CREATE_SUSPENDED, assigns, then resumes, and exposes `JobObjectChild::job_handle().terminate(code)` for tree kill. It uses windows 0.62.2, same major as Tauri 2.12.5 (`windows ^0.62`). win32job 2.0.3 (2025-05-15) has a simple API (Job::create, query_extended_limit_info, limit_kill_on_job_close, set_extended_limit_info, assign_process / assign_current_process) but depends on windows ^0.61, adding a second windows crate build. tauri-plugin-shell 2.6.0 added `set_process_group(true)` (2.5.0) and `kill_on_drop` (2.6.0), but its job_object.rs is documented 'Closing it leaves the processes in the job running' and cleanup runs only on RunEvent::Exit, i.e. not on a crash.

Source: https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information ; https://raw.githubusercontent.com/watchexec/process-wrap/main/src/windows.rs ; .../process-wrap/main/src/tokio/job_object.rs ; .../process-wrap/main/README.md ; https://raw.githubusercontent.com/ohadravid/win32job-rs/master/README.md ; https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/shell/src/process/job_object.rs

### macOS has no kill-on-parent-death primitive; process groups handle deliberate tree kills, and crash safety needs a watchdog helper (pipe-EOF and/or kqueue NOTE_EXIT).

Confidence: medium

Verified building blocks: tokio 1.54.1 `Command::process_group(0)` and unsafe `pre_exec` (unix), `kill_on_drop` (direct child only, and only when the handle is dropped in a live process); libc 0.2.191 exposes EVFILT_PROC, NOTE_EXIT, kevent, proc_listchildpids, proc_listpgrppids for Apple targets; nix 0.31.3 has a `Kqueue` type (feature `event`). Codex CLI's process_group.rs does setsid()/setpgid(0,0) in pre_exec, killpg(SIGTERM/SIGKILL) for teardown, and uses prctl(PR_SET_PDEATHSIG) on Linux only (no-op elsewhere); it also notes that on macOS denied group signals are retried against individual members. Design (my synthesis, not a verified recipe): start one reaper helper at launch (the app binary re-executed with a hidden flag) in its own session, ignoring SIGHUP/SIGINT/SIGTERM, connected by a pipe; the app sends it the PGIDs of llama-server and tool children; on pipe EOF or NOTE_EXIT for the app PID it sends killpg(SIGTERM), waits, then killpg(SIGKILL). Add a pidfile check at next launch that verifies the executable path before killing a stale llama-server. Children that call setsid themselves escape process-group kills.

Source: https://raw.githubusercontent.com/openai/codex/main/codex-rs/utils/pty/src/process_group.rs ; https://raw.githubusercontent.com/rust-lang/libc/0.2.191/src/unix/bsd/apple/mod.rs ; https://raw.githubusercontent.com/nix-rust/nix/v0.31.3/src/sys/event.rs ; https://raw.githubusercontent.com/tokio-rs/tokio/tokio-1.54.1/tokio/src/process/mod.rs

### For one-shot commands with streamed output use plain piped tokio::process; portable-pty 0.9.0 is usable but slow-moving and only needed for interactive TTY programs.

Confidence: medium

portable-pty 0.9.0 was published 2025-02-11 (previous 0.8.1 on 2023-03-13), lives in the wezterm repo, and is still what Codex CLI depends on (`portable-pty = "0.9.0"`), though Codex also carries its own Windows ConPTY wrapper and keeps a separate pipe path (`spawn_pipe_process`, `spawn_pipe_process_no_stdin`) with a 1 MiB default output cap (DEFAULT_OUTPUT_BYTES_CAP = 1024*1024). Alternative pty-process 0.5.4 (2026-10-19) is Unix-only by design (not verified in detail). With pipes: set stdin to null, read stdout/stderr concurrently, cap output, enforce a timeout, and kill the whole job/process group, because a surviving grandchild holding the pipe keeps the read open. On Windows set creation flag CREATE_NO_WINDOW so no console flashes (tauri-plugin-shell does the same). ConPTY requiring Windows 10 1809+ is from memory.

Source: https://crates.io/api/v1/crates/portable-pty ; https://raw.githubusercontent.com/openai/codex/main/codex-rs/utils/pty/src/lib.rs ; https://raw.githubusercontent.com/openai/codex/main/codex-rs/Cargo.toml ; https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/shell/src/process/mod.rs

### PowerShell: target powershell.exe (Windows PowerShell 5.1, always present) with `-NoLogo -NoProfile -NonInteractive -Command <script>`; pwsh.exe is optional. Known traps: `-Command` must be last, exit codes collapse to 0/1, `&&`/`||` do not exist in 5.1, and output is not UTF-8 by default.

Confidence: high

Microsoft about_PowerShell_exe (5.1): 'If the value of Command is a string, Command must be the last parameter'; the process exit code is 0 or 1 and other native exit codes are 'converted to 1' unless you append `exit $LASTEXITCODE`; -NonInteractive makes Read-Host/confirmation prompts 'statement terminating errors rather than hanging'; -EncodedCommand takes Base64 of a UTF-16LE string; `-Command -` reads the script from stdin. about_Pipeline_Chain_Operators: 'Beginning in PowerShell 7, PowerShell implements the && and || operators', so model-written `a && b` is a parse error in 5.1. about_Character_Encoding: in 5.1 `>`/Out-File write UTF-16LE and BOM-less scripts are read as ANSI; PowerShell 7 defaults to utf8NoBOM. Codex CLI prefixes every script with `try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n` (constant UTF8_OUTPUT_PREFIX), probes with `-NoLogo -NoProfile -Command "Write-Output ok"`, and notes that pwsh.exe 'must be installed separately' and may not be on PATH. From memory only: with -EncodedCommand/redirected streams 5.1 can emit '#< CLIXML' progress noise on stderr (mitigate with `$ProgressPreference='SilentlyContinue'`), and setting `$OutputEncoding` to [Text.Encoding]::UTF8 in 5.1 can send a BOM to native programs. Always strip a leading BOM and decode lossily.

Source: https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_powershell_exe?view=powershell-5.1 ; https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_pipeline_chain_operators?view=powershell-7.5 ; https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_character_encoding?view=powershell-7.5 ; https://raw.githubusercontent.com/openai/codex/main/codex-rs/shell-command/src/powershell.rs

### zsh: `/bin/zsh -lc '<cmd>'` reads zshenv, zprofile and zlogin but not ~/.zshrc; a Finder-launched GUI app does not inherit the user's terminal PATH, so capture the login-shell environment once.

Confidence: high

zsh docs (Files): commands are read from zshenv and $ZDOTDIR/.zshenv always; 'If the shell is a login shell, commands are read from zprofile and then $ZDOTDIR/.zprofile. Then, if the shell is interactive, commands are read from zshrc and then $ZDOTDIR/.zshrc. Finally, if the shell is a login shell, zlogin and $ZDOTDIR/.zlogin are read.' So `-lc` picks up Homebrew's shellenv when it is in .zprofile but misses nvm/pyenv/conda set up in .zshrc; `-c` alone gets only .zshenv. Codex CLI recognises `bash|zsh|sh` with `-lc` or `-c`. Tauri's fix-path-env-rs (git-only, not on crates.io) runs `$SHELL -ilc "echo -n _SHELL_ENV_DELIMITER_; env; echo -n _SHELL_ENV_DELIMITER_; exit"` with DISABLE_AUTO_UPDATE=true, defaulting to /bin/zsh on macOS, strips ANSI escapes and then calls std::env::set_var. Better: do the same capture once at startup with a timeout, keep the result in a map and pass it per child via `.envs()`, then run tool commands with non-interactive `zsh -lc`. That GUI apps start with a minimal launchd PATH is from memory.

Source: https://raw.githubusercontent.com/zsh-users/zsh/master/Doc/Zsh/files.yo ; https://raw.githubusercontent.com/tauri-apps/fix-path-env-rs/dev/src/lib.rs ; https://raw.githubusercontent.com/openai/codex/main/codex-rs/shell-command/src/bash.rs

### Shell-parsing crates: tree-sitter-bash 0.25.1 (+ tree-sitter 0.27.2) and tree-sitter-powershell 0.26.6 are the practical choice; brush-parser 0.4.0 is a real bash AST alternative; shlex/shell-words only split words; conch-parser is dead; yash-syntax is GPL.

Confidence: high

tree-sitter 0.27.2 (2026-11-07, MSRV 1.90) accepts grammar ABI 13-15 (api.h: TREE_SITTER_LANGUAGE_VERSION 15, MIN_COMPATIBLE 13); tree-sitter-bash 0.25.1 (2025-12-02) parser.c is ABI 15 and depends only on tree-sitter-language ^0.1, so the pair is compatible; Codex pins tree-sitter 0.25.10 + tree-sitter-bash 0.25 + tree-sitter-powershell =0.26.4. tree-sitter-powershell 0.26.6 (2026-10-28) is from airbus-cert. tree-sitter is error-tolerant, so `root.has_error()` must be treated as 'unparseable'. brush-parser 0.4.0 (2026-05-03, MSRV 1.88, MIT; parser of the brush shell) has heavier dependencies (peg, cached, bon, and insta listed as a normal dependency). shlex 2.0.1 (2026-05-17) and shell-words 1.1.1 (2025-12-10) do POSIX word splitting only and know nothing about `;`, `|`, `&&`, `$()`, redirections, so they are insufficient alone. conch-parser 0.1.1 was last published 2019-05-15. yash-syntax 0.25.0 is GPL-3.0-or-later.

Source: https://crates.io/api/v1/crates/{tree-sitter,tree-sitter-bash,tree-sitter-powershell,brush-parser,shlex,shell-words,conch-parser,yash-syntax} ; https://raw.githubusercontent.com/tree-sitter/tree-sitter/v0.27.2/lib/include/tree_sitter/api.h ; https://raw.githubusercontent.com/tree-sitter/tree-sitter-bash/v0.25.1/src/parser.c

### Codex CLI (open source, Rust) combines a small dangerous-command matcher, a strict tree-sitter 'plain commands only' parser and a prefix-rule policy engine, and explicitly relies on an OS sandbox rather than on the heuristics.

Confidence: high

Current paths: codex-rs/shell-command/src/command_safety/is_dangerous_command.rs, windows_dangerous_commands.rs, bash.rs, powershell.rs; decisions in codex-rs/core/src/exec_policy.rs; rules in codex-rs/execpolicy (`prefix_rule(pattern=[...], decision=allow|prompt|forbidden)`, strictest match wins). The old core/src/command_safety/is_safe_command.rs path returns 404 on main; I did not find where a safe-list lives now. Dangerous matcher: `rm` with -f/--force (ForcedRm), recursion through `sudo`, `env`, `trap`, and through `bash|zsh|sh -lc/-c` scripts via parse_shell_lc_literal_commands, with MAX_DANGEROUS_COMMAND_WRAPPER_DEPTH = 8 (exceeding it counts as dangerous). Windows: cmd `del|erase /f`, `rd|rmdir /s /q`; PowerShell delete cmdlets (remove-item, ri, rm, del, erase, rd, rmdir) with -Force; Start-Process/saps/Invoke-Item/ii; URL/browser launches. try_parse_word_only_commands_sequence accepts only node kinds program, list, pipeline, command, command_name, word, string, string_content, raw_string, number, concatenation and operators `&&`, `||`, `;`, `|`; anything else (redirections, substitutions, subshells, control flow, variables) returns None. The literal-command extractor is documented as 'suitable for identifying dangerous literal commands, but must not be used to prove that a command is safe'. exec_policy.rs: 'If the command is flagged as dangerous or we have no sandbox protection, we should never allow it to run without approval.'

Source: https://raw.githubusercontent.com/openai/codex/main/codex-rs/shell-command/src/command_safety/is_dangerous_command.rs ; .../shell-command/src/command_safety/windows_dangerous_commands.rs ; .../shell-command/src/bash.rs ; .../core/src/exec_policy.rs ; .../execpolicy/README.md

### git2 0.21.0 (2026-05-18) changed defaults: `ssh`, `https` and `cred` are no longer enabled; on Windows and macOS the `https` feature needs no OpenSSL.

Confidence: high

Changelog 0.21.0: 'Previously default = ["ssh", "https"]; now default = []'; CredentialHelper and the url dependency are behind the new `cred` feature (pulled in by `https`/`ssh`). Features: cred, https, ssh, vendored-libgit2, vendored-openssl, zlib-ng-compat, unstable, unstable-sha256. libgit2-sys 0.18.8+1.9.7 (2026-08-21) vendors libgit2 1.9.7; its build.rs defines GIT_WINHTTP on Windows, GIT_SECURE_TRANSPORT on Apple targets and GIT_OPENSSL elsewhere, so `vendored-openssl` is irrelevant for the two shipping targets. The vendored copy is linked statically when `vendored-libgit2` is set (or no suitable system libgit2 exists) unless LIBGIT2_NO_VENDOR=1. Suggested line: `git2 = { version = "0.21", default-features = false, features = ["vendored-libgit2", "https"] }`. Many string accessors changed from Option<&str> to Result in 0.21, so older example code needs adjusting.

Source: https://raw.githubusercontent.com/rust-lang/git2-rs/main/CHANGELOG.md ; https://raw.githubusercontent.com/rust-lang/git2-rs/git2-0.21.0/Cargo.toml ; https://raw.githubusercontent.com/rust-lang/git2-rs/git2-0.21.0/README.md ; https://raw.githubusercontent.com/rust-lang/git2-rs/main/libgit2-sys/build.rs

### git2 covers clone/fetch/status/diff/commit/push over HTTPS without system git, but credentials must come from the app: git credential helpers only work when helper programs are installed.

Confidence: medium

Verified API: Cred::userpass_plaintext(user, token), Cred::credential_helper(&config, url, username) / CredentialHelper (parses `credential.helper` and 'invoke[s] the necessary processes', so it depends on helpers such as Git Credential Manager or osxkeychain existing on the machine), Cred::default() for Negotiate/NTLM, FetchOptions::depth(i32) for shallow clones, proxy_options, follow_redirects, custom_headers, and unsafe opts::set_server_connect_timeout_in_milliseconds / set_server_timeout_in_milliseconds / set_verify_owner_validation. So: store a PAT in the OS keychain and return it from RemoteCallbacks::credentials. There is no single 'pull' call (fetch + merge analysis + fast-forward or merge), and from memory libgit2 runs no hooks, has no LFS filters and no gc, and enforces repository-ownership checks that surface as 'not owned by current user' errors on Windows.

Source: https://raw.githubusercontent.com/rust-lang/git2-rs/git2-0.21.0/src/cred.rs ; .../git2-0.21.0/src/opts.rs ; .../git2-0.21.0/src/remote.rs

### gix (gitoxide) 0.90.0 (2026-10-22, MSRV 1.88) is not ready as the only engine: push is still unimplemented, as are checkout/switch/reset and merge workflow orchestration.

Confidence: high

crate-status.md on main has unchecked items '[ ] push and self-contained clone/fetch over file:// and ssh://', '[ ] checkout, switch, restore and reset', '[ ] merge, cherry-pick and revert', '[ ] rebase', '[ ] client-side hooks', '[ ] sparse checkout support'; clone/fetch over HTTP, status, diff, merge-base and low-level checkout of an index are done. Codex CLI uses gix 0.89.0 only with `default-features = false, features = ["sha1"]`. The sensible single fallback is therefore a detected system `git` executable (which 8.0.6 can locate it), not gix.

Source: https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/crate-status.md ; https://crates.io/api/v1/crates/gix ; https://raw.githubusercontent.com/openai/codex/main/codex-rs/Cargo.toml

### PDF text extraction options: pdf-extract 0.12.1 (pure Rust, small, can panic), pdf_oxide 0.3.78 (pure Rust, young, strong self-reported numbers), pdfium-render 0.9.5 (best fidelity, needs a bundled PDFium binary); lopdf 0.47.0 is a low-level library.

Confidence: high

pdf-extract 0.12.1 (2026-09-16, MIT, jrmuizel): extract_text, extract_text_from_mem, extract_text_by_pages, extract_text_from_mem_by_pages and *_encrypted variants; depends on lopdf ^0.42 (five minors behind lopdf 0.47.0 of 2026-11-04); src/lib.rs contains 31 `panic!` and 42 `unwrap()` call sites, so run it under catch_unwind on a blocking thread with size/time limits. pdf_oxide 0.3.78 (2026-09-08, MIT OR Apache-2.0, MSRV 1.88): first published 2025-11-05, ~78 releases in a year; `PdfDocument::open(path)?.extract_text(page)?` and `to_markdown`; README claims 100% pass on 3,830 PDFs and lists pdf_extract at 91.5% and lopdf at 80.2% (vendor benchmark, not independently verified); non-optional deps include image, taffy, office_oxide, env_logger, quick-xml. pdfium-render 0.9.5 (2026-11-01): `pdfium_latest` = pdfium_8125; dynamic binding via `Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))`, or `static` feature; 'will not build Pdfium for you'; prebuilt assets from bblanchon/pdfium-binaries are named pdfium-win-x64.tgz and pdfium-mac-arm64.tgz; access is serialised behind a mutex (`thread_safe` default). mupdf 0.8.0 is AGPL-3.0 and unsuitable.

Source: https://raw.githubusercontent.com/jrmuizel/pdf-extract/master/src/lib.rs ; https://crates.io/api/v1/crates/pdf-extract/0.12.1/dependencies ; https://raw.githubusercontent.com/yfedoseev/pdf_oxide/main/README.md ; https://raw.githubusercontent.com/ajrcarey/pdfium-render/master/README.md ; https://raw.githubusercontent.com/bblanchon/pdfium-binaries/master/README.md

### Encoding detection: chardetng 1.0.0 (2026-03-30) + encoding_rs 0.8.44 (2026-10-30); the 1.0 API changed and UTF-16 must be handled by a BOM check first.

Confidence: high

chardetng 1.0.0 API: `EncodingDetector::new(Iso2022JpDetection)`, `feed(&[u8], last: bool) -> bool`, `guess(tld: Option<&[u8]>, allow_utf8: Utf8Detection) -> &'static Encoding`; release notes: boolean replaced by the two-variant Utf8Detection enum, `guess_assess` removed, ISO-2022-JP control added. README: UTF-16BE/LE are 'Not detected: Detecting these belongs on the BOM layer', and 'Improvements to detection results are not planned'. Pipeline: BOM sniff (encoding_rs `Encoding::for_bom`, from memory) -> try strict UTF-8 -> chardetng on the first chunk -> encoding_rs decode. This matters on Windows because PowerShell 5.1 `>` writes UTF-16LE. Codex still pins chardetng 0.1.17, so its call sites show the old signature.

Source: https://raw.githubusercontent.com/hsivonen/chardetng/master/src/lib.rs ; https://raw.githubusercontent.com/hsivonen/chardetng/master/README.md ; https://crates.io/api/v1/crates/encoding_rs

### jsonschema 0.61.0 (2026-11-08, MSRV 1.85, MIT): supports Drafts 4, 6, 7, 2019-09, 2020-12; compile once with `jsonschema::validator_for(&schema)?`; its default features enable network and file $ref resolution and must be turned off for an offline app.

Confidence: high

Default features: resolve-http, resolve-file, tls-aws-lc-rs, idna (resolve-http pulls reqwest + rustls). Use `jsonschema = { version = "0.61", default-features = false }`. API from README: `jsonschema::is_valid(&schema, &instance)` one-off; `let validator = jsonschema::validator_for(&schema)?;` then `validator.is_valid(&instance)`, `validator.iter_errors(&instance)`, `validator.evaluate(&instance)`. Cache one Validator per MCP tool inputSchema. Releases are frequent (0.x), so pin the minor.

Source: https://crates.io/api/v1/crates/jsonschema ; https://raw.githubusercontent.com/Stranger6667/jsonschema/master/README.md

### zip 9.5.0 (2026-11-09, MSRV 1.88, MIT, repo zip-rs/zip2): `ZipFile::enclosed_name()` is the zip-slip guard and `ZipArchive::extract()` now uses it and constrains symlinks; similar 3.2.1 (2026-10-24, MSRV 1.85, Apache-2.0) for diffs.

Confidence: high

enclosed_name() -> Option<PathBuf>: rejects NUL bytes, absolute paths and paths resolving outside the root ('foo/../bar is fine, foo/../../bar is not'); mangled_name() is explicitly the weaker option. extract(): 'Paths are sanitized with ZipFile::enclosed_name', symlinks 'are only created, and only followed, if their target resolves to a path inside the destination directory', not atomic and not safe against concurrent modification of the destination; ExtractOptions/SymlinkExtractAction can omit symlinks. Default features pull many codecs (aes-crypto, bzip2, deflate64, deflate, lzma, ppmd, time, zstd, xz); for skill archives use `default-features = false, features = ["deflate"]` and add your own entry-count and uncompressed-size caps (zip bombs are not handled for you). similar 3.0 added Algorithm::Histogram and Hunt, accepts owned inputs in TextDiff::from_*, and removed old_slices/new_slices; default features std + text; Codex is still on 2.7.0.

Source: https://static.crates.io/crates/zip/zip-9.5.0.crate (src/read/zipfile.rs, src/read/extract.rs) ; https://crates.io/api/v1/crates/zip ; https://raw.githubusercontent.com/mitsuhiko/similar/main/CHANGELOG.md

### YAML after serde_yaml's deprecation: serde-saphyr 2.1.1 (2026-11-05) is the actively maintained, unsafe-free choice; serde_yml must be avoided; serde_norway and serde_yaml_ng are stale forks.

Confidence: high

serde_yaml 0.9.34+deprecated (2024-03-25). serde_yml: RUSTSEC-2025-0068 'serde_yml crate is unsound and unmaintained' (and RUSTSEC-2025-0067 for libyml); 0.0.13 (2026-05-27) describes itself as 'DEPRECATED ... a thin compatibility shim that forwards every call to noyalib'. serde_norway 0.9.42 (2024-12-21) and serde_yaml_ng 0.10.0 (2024-05-26) have no advisories on rustsec.org but no releases for ~2 years and still sit on unsafe-libyaml lineage (RUSTSEC-2024-0449 marks unsafe-libyaml unmaintained). serde-saphyr 2.1.1 (MSRV 1.89, MIT OR Apache-2.0, first published 2025-09-27, 13.4M recent downloads): unsafe denied, fuzz-tested 'designed not to panic on malformed YAML', configurable `Budget` against resource-exhaustion inputs, `serde_saphyr::from_str` / `from_str_with_options`; not a serde_yaml fork and not part of the saphyr project (saphyr 0.3.3 is a separate non-serde YAML library). Note it went 1.0 -> 2.0 within weeks (2026-10-13), so pin the major. For SKILL.md, split the `---` frontmatter by hand and deserialize into a typed struct; gray_matter 0.3.2 (2025-07-10) is unnecessary.

Source: https://rustsec.org/packages/serde_yml.html ; https://rustsec.org/packages/libyml.html ; https://rustsec.org/packages/unsafe-libyaml.html ; https://crates.io/api/v1/crates/serde_yml ; https://raw.githubusercontent.com/bourumir-wyngs/serde-saphyr/master/README.md

### Paths and search: dunce 1.0.5 for canonicalisation without \\?\ prefixes; ignore 0.4.34 + globset 0.4.21 + grep-searcher 0.1.17 / grep-regex 0.1.14 (ripgrep's libraries) for file walking and content search.

Confidence: high

dunce 1.0.5 (2024-08-04; CC0-1.0 OR MIT-0 OR Apache-2.0; old but feature-complete): `dunce::canonicalize(path)` is a drop-in for fs::canonicalize and `dunce::simplified(&Path)` strips the verbatim prefix only when safe ('\\?\C:\Windows will be converted to C:\Windows, but \\?\C:\COM will be left as-is'); no-op on non-Windows. canonicalize needs the path to exist, so for a not-yet-created file canonicalise the parent and re-join before the workspace-containment check. ignore 0.4.34 and globset 0.4.21 were both published 2026-10-27 (MSRV 1.88); grep-searcher 0.1.17 and grep-matcher 0.1.9 on 2026-07-15; grep-regex 0.1.14 on 2025-10-16; all Unlicense OR MIT from the ripgrep repo. Codex uses ignore 0.4.30, globset 0.4 and dunce 1.0.4.

Source: https://static.crates.io/crates/dunce/dunce-1.0.5.crate (README.md, src/lib.rs) ; https://crates.io/api/v1/crates/{ignore,globset,grep-searcher,grep-regex,grep-matcher} ; https://raw.githubusercontent.com/openai/codex/main/codex-rs/Cargo.toml

### Async/HTTP/hash picks: tokio-util 0.7.20 CancellationToken, reqwest 0.13.5 (rustls is now the default TLS and several features became opt-in), sha2 0.11.1.

Confidence: high

tokio-util 0.7.20 (2026-10-09, MSRV 1.85): `tokio_util::sync::CancellationToken` is ungated (pub mod sync) with new, child_token, cancel, is_cancelled, cancelled, cancelled_owned, drop_guard, run_until_cancelled. tokio 1.54.1 (2026-10-21). reqwest 0.13.5 (2026-09-08, MSRV 1.85): `default = [default-tls, charset, http2, system-proxy]`, `default-tls = [rustls]`, `rustls = aws-lc-rs provider + rustls-platform-verifier` (OS trust store, so corporate root CAs work); alternatives `rustls-no-provider` and `native-tls`; `json`, `stream`, `query`, `form`, `gzip`, `socks` are opt-in. aws-lc-rs on x86_64-pc-windows-msvc needs a C/C++ compiler and NASM (or its prebuilt-NASM option), which affects Windows CI. Tauri 2.12.5 itself only pulls reqwest ^0.13 on mobile targets. The default `system-proxy` feature reads OS proxy settings; add an explicit no-proxy rule for the 127.0.0.1 llama-server client. sha2 0.11.1 (2026-10-12, MSRV 1.88): 0.11.0 (2026-03-25) moved to digest 0.11, replaced type aliases with newtypes and removed the asm/std features, so 0.10-era snippets may need small changes. rustls stable is 0.23.46; rustls-platform-verifier 0.7.1.

Source: https://raw.githubusercontent.com/tokio-rs/tokio/tokio-util-0.7.20/tokio-util/src/sync/cancellation_token.rs ; https://crates.io/api/v1/crates/reqwest ; https://raw.githubusercontent.com/aws/aws-lc-rs/main/book/src/requirements/windows.md ; https://raw.githubusercontent.com/RustCrypto/hashes/master/sha2/CHANGELOG.md

### Images and logging: image 0.25.10 + fast_image_resize 6.1.0; tracing 0.1.44 + tracing-subscriber 0.3.23 + tracing-appender 0.2.5, whose rotation is time-based only.

Confidence: high

image 0.25.10 (2026-03-10, MSRV 1.88): default = rayon + default-formats (avif, bmp, dds, exr, ff, gif, hdr, ico, jpeg, png, pnm, qoi, tga, tiff, webp); use `default-features = false, features = ["png", "jpeg", "webp"]` to keep the binary small. fast_image_resize 6.1.0 (2026-07-21, MSRV 1.87): default = std; optional `image` feature for DynamicImage interop and `rayon`; worthwhile for screenshot downscaling in computer-use loops, otherwise image's own resize is enough. tracing-appender 0.2.5 (2026-04-17): `RollingFileAppender::builder().rotation(Rotation::DAILY).filename_prefix(..).filename_suffix(..).max_log_files(n).latest_symlink(..).build(dir)`; Rotation constants are MINUTELY, HOURLY, DAILY, WEEKLY, NEVER, with no size-based rotation; `tracing_appender::non_blocking(writer)` returns `(NonBlocking, WorkerGuard)` and the guard must be kept alive for the app's lifetime or buffered lines are lost. Secrets and prompt contents need explicit redaction since logs are plain local files.

Source: https://crates.io/api/v1/crates/{image,fast_image_resize,tracing,tracing-subscriber,tracing-appender} ; https://raw.githubusercontent.com/tokio-rs/tracing/tracing-appender-0.2.5/tracing-appender/src/rolling/builder.rs ; .../tracing-appender/src/rolling.rs ; .../tracing-appender/src/lib.rs

### Research limits: github.com HTML/API pages and zsh.sourceforge.io were blocked from this environment; nothing was compiled or run on Windows or macOS.

Confidence: high

All GitHub evidence came from raw.githubusercontent.com (main/master or a release tag), so 'main' findings for Codex, gitoxide, process-wrap and tauri plugins reflect the branch on 2026-11-10 rather than a tagged release (tauri-plugin-shell features were matched to its CHANGELOG entries for 2.5.0/2.6.0). The zsh startup-file text came from the zsh-users/zsh mirror. Apple's statement on ad-hoc signatures could not be loaded from a primary page. Items stated from memory are flagged inline: CLIXML stderr noise, ConPTY minimum Windows version, libgit2 hooks/LFS/gc gaps, GUI-app launchd PATH, rusqlite Connection Send/!Sync. The repository working directory was not touched; scratch files were kept in the session scratchpad.

Source: session tool output (HTTP 403 from github.com and zsh.sourceforge.io via the agent proxy)

## Recommendations

- SQLite: `rusqlite = { version = "0.40", features = ["bundled"] }` (resolves to 0.40.2, SQLite 3.53.2) + rusqlite_migration 2.6.0, and move to 0.41.x once rusqlite_migration/tokio-rusqlite accept it. Do not add sqlx or tauri-plugin-sql. Own the DB on one dedicated thread (tokio-rusqlite 0.8.0 or a small hand-rolled actor); set journal_mode=WAL, synchronous=NORMAL and a busy timeout at open, outside migrations.
- Chat search: external-content FTS5 table with `tokenize="trigram"` kept in sync by triggers; route queries shorter than 3 characters to a LIKE scan; add a unit test calling `MIGRATIONS.validate()`.
- Secrets: use `keyring = "4.2"` (default `v1`) for the simple path, or keyring-core 1.0.1 + apple-native-keyring-store (feature `keychain`) + windows-native-keyring-store (default-features = false) for control. Store one entry per secret via set_secret (UTF-8, <= 2560 bytes) and keep only references in SQLite. If long values or many macOS prompts become a problem, switch to one 32-byte master key in the keychain and encrypt secrets in SQLite. Serialise all keychain writes on one thread.
- Sign macOS builds with a stable identity from day one (Developer ID for releases, one self-signed cert for dev) and give dev builds a non-keychain store option, otherwise every rebuild re-prompts per item.
- Process control: spawn llama-server and all tool children yourself with tokio::process, not tauri-plugin-shell. Windows: process-wrap 10.0.1 (`features = ["tokio1"]`) with `.wrap(KillOnDrop).wrap(JobObject)` plus CreationFlags(CREATE_NO_WINDOW), or one app-wide job created with the `windows` 0.62 crate with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE to which every child is assigned while suspended. Do not assign the app's own process to that job, or an updater/installer it launches would be killed with it. Skip win32job (older windows crate) and command-group (superseded).
- macOS: spawn each child as a process-group leader (ProcessGroup::leader() / process_group(0)), tear down with killpg SIGTERM then SIGKILL after a grace period (with per-member fallback as Codex does), and add a reaper helper process that kills registered groups on pipe EOF / NOTE_EXIT of the app, plus pidfile-based stale-sidecar cleanup at startup that verifies the executable path before killing.
- Terminal tool: piped stdout/stderr, stdin null, concurrent readers, output cap (Codex uses 1 MiB), timeout, CancellationToken wired to tree kill. Windows: `powershell.exe -NoLogo -NoProfile -NonInteractive -Command <script>` with the UTF-8 console-encoding prefix and a trailing `exit $LASTEXITCODE`; tell the model in the tool description that it is PowerShell 5.1 (no `&&`/`||`); use pwsh.exe only if detected and the user opts in. macOS: `/bin/zsh -lc <script>` with an environment captured once from a login+interactive shell under a timeout.
- Command approval (fail closed): parse with tree-sitter-bash / tree-sitter-powershell; auto-run only when the parse has no errors, every node is in a plain-word allowlist (Codex's ALLOWED_KINDS), and every command in the sequence is on a small read-only allowlist with vetted flags. Everything else prompts: parse errors, redirections, substitutions, variables, subshells, unknown programs, wrappers (sudo, env, xargs, sh -c, Invoke-Expression, -EncodedCommand, Start-Process). Keep a separate destructive matcher (rm -f/-r, del /f, rd /s, Remove-Item -Recurse/-Force, git reset --hard, git clean, format, dd, mkfs, shutdown, chmod/chown -R, registry edits) whose hits always prompt and can never be covered by 'allow for session'. State in the docs that this is a UX guard, not a security boundary.
- Git: `git2 = { version = "0.21", default-features = false, features = ["vendored-libgit2", "https"] }` as the built-in engine with PAT credentials from the keychain via RemoteCallbacks::credentials; the single fallback is a detected system `git` CLI for SSH remotes, credential-helper/OAuth flows, LFS and hooks. Do not adopt gix until push lands.
- PDF: put extraction behind a trait, run it on a blocking thread under catch_unwind with page/size/time limits. Start with pdf-extract 0.12.1; benchmark pdf_oxide 0.3.78 on your own sample set before trusting its claims; keep pdfium-render 0.9.5 with bundled pdfium-win-x64 / pdfium-mac-arm64 binaries as the high-fidelity option if pure-Rust quality is insufficient.
- Text files: BOM check -> strict UTF-8 -> chardetng 1.0.0 guess -> encoding_rs decode; refuse or hex-summarise binaries.
- Misc pins: jsonschema 0.61 with default-features = false (one cached Validator per tool schema); zip 9.5 with default-features = false, features ["deflate"], always enclosed_name() plus your own size/count caps; serde-saphyr 2.1 for SKILL.md frontmatter; dunce 1.0.5 for every path shown to or compared for the user; ignore + globset + grep-searcher for search tools; similar 3.2 for diffs; sha2 0.11 for model checksums; tracing-appender 0.2.5 with Rotation::DAILY and max_log_files, holding the WorkerGuard in app state.
- HTTP: reqwest 0.13 with default rustls + platform verifier for model downloads (enable `stream`, `json` explicitly); confirm the Windows CI image has NASM for aws-lc-rs or choose `rustls-no-provider` with ring / `native-tls`; build the localhost llama-server client with proxies disabled.

## Risks

- Version lag: rusqlite 0.41.x is current but rusqlite_migration, tokio-rusqlite, deadpool-sqlite and refinery cap at 0.40; an unpinned `cargo update` or adding sqlx/tauri-plugin-sql later will fail to resolve because of the libsqlite3-sys `links` constraint.
- Windows Credential Manager's 2560-byte blob limit silently caps secret size (1280 UTF-16 units via set_password); large tokens or JSON credentials return TooLong. Default Enterprise persistence can roam secrets with domain roaming profiles, and concurrent credential writes can lose updates.
- macOS keychain prompts: ad-hoc or unsigned builds trigger a password prompt per keychain item after every rebuild; with one item per MCP secret this can be dozens of prompts, and users may click Deny, which must be handled as a normal error.
- Process cleanup is not airtight: tauri-plugin-shell's job object does not kill on crash; process-wrap's JobObject without KillOnDrop does not either; on macOS a child that calls setsid escapes group kills and the reaper helper itself can be killed, so an orphaned llama-server holding the port and GPU memory remains possible. The reaper design is my synthesis and needs real testing with force-quit and crash scenarios on both OSes.
- PowerShell 5.1 differences (no `&&`/`||`, exit code collapsed to 0/1, UTF-16LE redirection, OEM/ANSI output, possible CLIXML noise on stderr) will produce confusing tool results for local models unless normalised; some of these details are from memory and need a Windows test matrix.
- Destructive-command detection by parsing is heuristic. Codex relies on an OS sandbox as the real boundary; this app has none on Windows/macOS by default, so interpreters and scripts (python -c, node -e, a script file the model just wrote, make targets, git aliases/hooks) can do anything after a single approval.
- libgit2 gaps: no credential-helper binaries without a Git install, no LFS smudge, no hooks, manual pull logic, and repository-ownership validation errors on Windows; behaviour differs from what users expect from `git`.
- PDF: pdf-extract has many panic paths and lags lopdf; pdf_oxide is one year old with ~78 releases and vendor-reported quality numbers; pdfium-render adds a native binary that must be bundled, code-signed/notarized on macOS and kept matched to the `pdfium_XXXX` feature.
- Supply-chain/offline posture: jsonschema's default features enable HTTP and file $ref retrieval; reqwest's default system-proxy can route even localhost calls through a proxy; serde_yml is RUSTSEC-flagged and now forwards to another crate; fast-moving 0.x/major-churn crates (jsonschema 0.61, zip 9.x, serde-saphyr 2.x, pdf_oxide) need pinned versions and cargo-deny/audit in CI.
- Evidence caveats: several findings come from default-branch source rather than tagged releases, github.com and zsh.sourceforge.io pages were unreachable (raw mirrors used), and nothing was built or executed on the target platforms.
