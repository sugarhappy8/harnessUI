# huggingface — re-check

An independent second pass over the research file of the same name. Where the two disagree, this file is the later and better-checked one.

## Corrections

### Search parameters: sort values, default sort trendingScore, invalid sort gives 400, limit clamps at 1000, direction=1 rejected for downloads and likes, expand[] value list.

Status: corrected

All confirmed except one detail: direction=1 is also rejected (400) for sort=lastModified; it is accepted for createdAt. So ascending sort is effectively unavailable for the three sorts a browser would use. The expand value list matches the server's 400 message exactly. apps=llama.cpp, num_parameters=min:6B,max:10B and gated=true all behaved as described.

Source: curl against /api/models with sort=bogus, direction=1, limit=1001, expand[]=bogus

### Allowlist: only huggingface.co and us.aws.cdn.hf.co were needed from a US egress; other regional hosts unverified.

Status: corrected

The CDN host depends on the repo's storage region, not only the client's location. From the same US egress, 5 of 6 GGUF repos tagged region:eu (jinaai, lightonai) redirected to https://cas-bridge.xethub-eu.hf.co/xet-bridge-eu/..., which also honours Range. A two-host allowlist would fail those downloads for every user. us.gcp.cdn.hf.co, cas-bridge.xethub.hf.co and cas-bridge.xethub-eu.hf.co all resolve in DNS; eu.aws.cdn.hf.co does not. All 300 most-downloaded GGUF repos are region:us, so the seed list is unaffected, but the free browser is.

Source: curl on /api/models?filter=gguf&filter=region:eu and resolve HEADs; getent hosts

### A valid token that has not been granted access gets 403 with GatedRepo and a 'not in the authorized list' message.

Status: unverifiable

No token was available to me either. The huggingface_hub GatedRepoError docstring on main shows exactly this 403 message, so the claim is consistent with client source but not observed. Must be tested with a real account before the error mapping is finalised.

Source: https://raw.githubusercontent.com/huggingface/huggingface_hub/main/src/huggingface_hub/errors.py

### The quant regex works in the Rust regex crate and matched 96.9% of main model files in the 2,000 most-downloaded GGUF repos.

Status: corrected

It compiles and runs under the regex crate (I built it). On my own 1,000-repo sample it matched 84.0% raw, 97.5% after dropping the five worst non-LLM repos, and 97.25% on repos filtered with the conversational tag, so the headline figure holds only after excluding non-model repos. About 10% of chat repos have at least one unlabelled file. One bug: because the pattern consumes the trailing separator, two adjacent labels sharing one separator cannot both match, so 'take the last match' returns the first one (x.Q4_K_M.Q8_0 gives Q4_K_M). Tokenise on separators or restart the search at the end of the capture group.

Source: Own corpus from /api/models?filter=gguf&sort=downloads&limit=1000&expand[]=siblings; cargo run with regex 1.x

### Gemma 4 is tagged apache-2.0 but links to a separate Gemma 4 licence page, so it may not really be Apache-2.0.

Status: corrected

The linked page (ai.google.dev/gemma/docs/gemma_4_license) contains the Apache License 2.0 text itself, with no Gemma terms of use or prohibited-use policy. The Apache-2.0 label is accurate; this risk can be dropped.

Source: https://ai.google.dev/gemma/docs/gemma_4_license

### Suggested tier defaults: 16 GB gpt-oss-20b MXFP4, 24 GB Qwen3.8-27B UD-Q4_K_M, 32+ GB Qwen3.6-35B-A3B Q4_K_M (placement by file size).

Status: corrected

The files exist, but the placement is not supportable as defaults. Weights plus mmproj alone are 12.1 GB on a 16 GB tier, 17.4 GB on 24 GB and 21.2 GB on 32 GB, before KV cache, compute buffers and the OS. On Apple Silicon the GPU working-set budget is a fraction of unified memory (commonly reported as two-thirds to three-quarters; I could not confirm the exact ratio from a primary source). llama.cpp master defaults n_ctx to 0 (the model's trained context, 262,144 here) and relies on --fit to shrink it, with a 4,096 floor. Treat tiers as untested until each model is loaded on real 8/16/24/32 GB machines.

Source: File sizes from the Hub API; llama.cpp master common/common.h and common/arg.cpp (n_ctx, --fit, --fit-ctx)

## Things the research missed

### Redirect hosts vary by repo storage region, so the allowlist needs a suffix rule, not two exact hosts.

region:eu repos redirect to cas-bridge.xethub-eu.hf.co from any client. Use: API, resolve and token only on exact host huggingface.co; redirect targets allowed when HTTPS and host is huggingface.co or ends in .hf.co. Ship the known hosts (us.aws.cdn.hf.co, cas-bridge.xethub-eu.hf.co, cas-bridge.xethub.hf.co, us.gcp.cdn.hf.co) as data for diagnostics. Never log the signed Location URL; for gated repos it is a one-hour bearer capability.

Source: Live resolve HEADs on region:eu repos; DNS probes

### Hugging Face OAuth supports public clients, device-code flow and a gated-repos scope, which is a better fit than pasting a personal access token.

The docs describe OAuth apps without a client secret, PKCE with loopback redirect on any port, and a device flow (POST https://huggingface.co/oauth/device, then /oauth/token). Scope gated-repos is 'Read the content of public gated repos the user has been granted access to' and excludes private repos. The app could request only that scope instead of holding a broad read token. The brief covers only PAT paste. Token paste should remain as a fallback.

Source: https://huggingface.co/docs/hub/oauth

### The llama-server sidecar can reach Hugging Face on its own and reads HF_TOKEN from the environment.

llama-server has -hf/--hf-repo, -hft (default from HF_TOKEN), a model endpoint override, a web UI, and an --offline flag ('forces use of cache, prevents network access', env LLAMA_ARG_OFFLINE). To keep 'fully offline' and the allowlist meaningful, start it with --offline, local -m and --mmproj paths, --no-webui, and a scrubbed environment with no HF_TOKEN or HF_ENDPOINT. The token must never reach the sidecar, MCP children or the terminal tool's environment, and should not be written to the standard ~/.cache/huggingface/token path that agent file tools can read.

Source: llama.cpp master common/arg.cpp lines 3062-3084, 3478-3479, 3968-3973

### The browser needs a concrete 'chat LLM' filter, and pipeline_tag is the wrong one.

Of the 300 most-downloaded GGUF repos, 60 have no pipeline_tag (including unsloth/Qwen3.8-27B-GGUF and all Granite 4.2 repos) and about 50 are speech, image or video models. The 'conversational' tag is present on 217 of 300 and on 38 of the 60 without a pipeline tag. filter=gguf&filter=conversational returned 100 of 100 matching repos. Combine it with gguf.architecture from the detail call to warn, before a multi-gigabyte download, when the bundled llama-server build does not support that architecture.

Source: Own measurement on /api/models?filter=gguf&sort=downloads&limit=300 and filter=conversational

### Where the HTTP calls run is an open decision: the Hub allows CORS from Tauri origins.

huggingface.co echoes any Origin (tauri://localhost, http://tauri.localhost) including an Authorization preflight, and the CDN sends access-control-allow-origin: *. So the webview could call the Hub directly, which would put the token in JavaScript and force a wide CSP connect-src. Keep all Hub traffic in Rust commands, leave connect-src closed to remote hosts, and do not add tauri-plugin-http (it exposes fetch to the webview and would compile a second reqwest, 0.12, beside the 0.13 used by rmcp).

Source: curl with Origin headers; crates.io dependency data for tauri-plugin-http 2.8.3 and rmcp 3.9.0

### Resume safety details that the brief leaves out.

The CDN ignores If-Range, so the only guards are: re-resolve on every attempt, compare x-linked-etag and x-linked-size with the values stored beside the partial, require 206 with a matching Content-Range total, and hash the whole file at the end. huggingface_hub also sends Accept-Encoding: identity and restarts the file if a ranged request returns 200 (it notes CloudFront can ignore Range when compression is negotiated). Set that header explicitly, because Cargo feature unification can switch on reqwest's gzip support.

Source: Ranged GETs with bogus If-Range; huggingface_hub main file_download.py lines 403-404, 1667

### Model cards and scan status are untrusted remote content.

README.md from arbitrary repos contains HTML and remote images; rendering it in the webview without sanitising and without blocking remote images would cause third-party requests, which conflicts with the no-telemetry promise, and is an injection surface next to agent tools. Separately, tree?expand=true returns securityFileStatus per file (Protect AI and antivirus results), which the app can show for user-chosen repos.

Source: Live /api/models/{repo}/tree/main?expand=true output; OpenAPI spec

### The curated schema lacks the fields needed to make tiering and compatibility enforceable.

Add per model: a measured memory figure at recommended_ctx (weights + mmproj + KV), the minimum llama.cpp build that loads the architecture and has the tool handler, and the tested build and date. Without these, the 16/24/32 GB defaults cannot be checked and a llama.cpp downgrade silently breaks qwen35, gemma4 or granite models. Always pass -c explicitly from recommended_ctx.

Source: llama.cpp master common/common.h (n_ctx = 0 default); seed file sizes

### The Hub endpoint should be configurable.

llama.cpp and hf-hub both honour an endpoint override (HF_ENDPOINT / model endpoint) because huggingface.co is unreachable on some networks and mirrors such as hf-mirror.com are in common use. If the app supports this, the endpoint must be an explicit user setting, off by default, and the allowlist must be derived from it rather than read from the environment.

Source: llama.cpp common_get_model_endpoint; hf-hub 1.0.0 src/constants.rs

## Final recommendations

- Keep the core design: hand-rolled reqwest 0.13 in Rust, a no-redirect client for huggingface.co plus an auth-free client for the CDN, pinned 40-hex commits, and a full SHA-256 check before the atomic rename. All of the API, header and hash facts it rests on reproduced. Do not adopt hf-hub 1.0.0; it forces the Xet protocol with no off switch.
- Replace the two-host allowlist with: exact huggingface.co for API, resolve and credentials; redirect targets accepted only over HTTPS when the host is huggingface.co or ends in .hf.co. Repos stored in the EU redirect to cas-bridge.xethub-eu.hf.co for every user. Never send Authorization off huggingface.co and never log signed URLs.
- Do not ship the proposed 16, 24 and 32 GB defaults as they stand. Re-tier after loading each model on real hardware of both platforms, using weights + mmproj + KV at a fixed context as the budget. Until then the safer picks from the verified list are Ministral-3-14B Q4_K_M for 16 GB and Gemma-4-31B QAT for 32 GB; a 24 GB default needs measurement. Always pass -c explicitly.
- Launch llama-server with --offline, local model paths, --no-webui and an environment stripped of HF_TOKEN and HF_ENDPOINT, so the app's downloader is the only component that talks to Hugging Face.
- Offer 'Sign in with Hugging Face' (public OAuth client, device-code or loopback PKCE, scope gated-repos) as the primary auth path, with token paste as fallback. Keep credentials in the OS keychain and out of the webview, the sidecar and agent tool environments.
- Filter the in-app browser with filter=gguf&filter=conversational, not pipeline_tag, and gate downloads on gguf.architecture being supported by the bundled llama.cpp build.
- Keep all Hub traffic in Rust commands, keep the CSP closed to remote hosts, skip tauri-plugin-http, and sanitise model-card markdown with remote images blocked.
- Harden resume: re-resolve on every attempt, compare x-linked-etag and x-linked-size with the stored values, require 206 with a matching Content-Range total, send Accept-Encoding: identity, and do not rely on If-Range.
- Fix the quant-label extraction for adjacent labels (tokenise on separators) and keep the quant and sidecar tables as updatable data.
- Extend the curated JSON with measured memory at recommended_ctx, minimum llama.cpp build, and the build and date the tool-call round trip was tested on. Gemma 4 may be labelled Apache-2.0.
- Before release, close the three things nobody could verify: the 403 path with a real token on a gated repo, a real tool-call round trip per seed model on the pinned llama.cpp build (newest tag today is b12242), and a download from an EU network and of an EU-region repo.

## Claims confirmed unchanged

- Search must use filter=gguf; library=gguf is silently ignored and returns the unfiltered trending list. — Confirmed live: library=gguf&limit=3 returned the same three non-GGUF repos as no filter; at limit=50 only 17 of 50 results carried the gguf tag, versus 50 of 50 with filter=gguf. Unknown parameters give no error.
- List/detail response shapes: default fields, full=true additions, expand[]=gguf block ({total, architecture, context_length, chat_template, ...}), Link-header cursor pagination with no total count. — Confirmed. unsloth/Qwen3.8-27B-GGUF returned total=27320697856 (parameters), architecture=qwen35, context_length=262144, a 9,993-char chat_template and totalFileSize=16056478688 (not the size of the recommended file). The next-page URL rewrites expand[]=x to expand=x and works when followed verbatim. /api/models and /api/models/{repo} are absent from the OpenAPI spec (3.1.0, 308 paths).
- ?blobs=true gives per-file size and lfs.sha256; tree API gives the same hash as lfs.oid; X-Linked-ETag is the content SHA-256; gated repos mask tree hashes anonymously but blobs=true does not. — I independently downloaded imatrix_unsloth.gguf (13,642,656 bytes): sha256sum = 0ee5b10b...f1c1, equal to x-linked-etag and lfs.sha256. On google/gemma-3-4b-it-qat-q4_0-gguf (gated: manual) the anonymous tree shows asterisks for lfs.oid and xetHash while ?blobs=true returns real sha256 values. Tree limit default 1,000 (100 with expand) is in the OpenAPI spec; expand=true adds lastCommit and securityFileStatus.
- /resolve/{rev}/{file} returns 302 to a one-hour signed URL on us.aws.cdn.hf.co; the CDN supports Range (206, 416 at EOF); CDN ETag is the Xet hash; non-LFS files 307 to /api/resolve-cache on the same host. — Confirmed: Expires was exactly 3,600 s after the response Date; headers x-repo-commit, x-linked-size, x-linked-etag, x-xet-hash and accept-ranges present, no plain etag on the 302. Range 0-1023 gave 206; a 16-byte range at offset 12,000,000,000 of the 16.46 GB seed file gave 206 with the correct total; a range starting at file size gave 416. One addition: the CDN ignores If-Range (a bogus validator still returned 206), so If-Range cannot protect a resume.
- reqwest 0.13.5 is current, strips Authorization/Cookie on cross-host redirects, and defaults to Policy::limited(10). — cargo search shows reqwest 0.13.5; src/redirect.rs remove_sensitive_headers removes Authorization, Cookie, cookie2, Proxy-Authorization and WWW-Authenticate on host, port or scheme change; default is Policy::limited(10). The CDN returned 206 with a bogus Authorization header attached. Related facts the brief did not state: reqwest 0.13's default TLS is rustls with aws-lc-rs and the platform verifier; rmcp 3.9.0 depends on reqwest ^0.13.2 (optional), while tauri-plugin-http 2.8.3 depends on reqwest ^0.12.16.
- Gated file anonymously: 401 with X-Error-Code: GatedRepo (same with an invalid token); missing file 404 EntryNotFound; bad revision 404 RevisionNotFound; nonexistent/private repo 401 without X-Error-Code; invalid tokens ignored on public endpoints, only /api/whoami-v2 returns 401. — All reproduced exactly, including identical 401 GatedRepo for anonymous and bogus-token requests, and 200/302 on public endpoints with a bogus token. Gated repo metadata and README are readable anonymously. HF docs state 'Requesting access can only be done from your browser.' huggingface_hub also maps a 'DisabledRepo' case (X-Error-Message 'Access to this resource is disabled.') that the brief's error table omits.
- Anonymous rate limits are 500 API and 3,000 resolver requests per 5-minute fixed window per IP, reported in RateLimit / RateLimit-Policy headers; no User-Agent required. — Headers observed: ratelimit-policy "fixed window";"api";q=500;w=300 and "resolvers";q=3000;w=300. The docs table (September '25) gives Anonymous 500 / 3,000 / 100 per IP and Free user 1,000 / 5,000 / 200, marked subject to change. Requests with no User-Agent returned 200 and 302. The docs also advise replacing API calls with resolver calls where possible.
- Split files use -NNNNN-of-NNNNN.gguf; sidecars are mmproj (substring), mtp-/dflash-/eagle3-/dspark- (prefix) and imatrix (substring); llama.cpp defaults to Q4_K_M then Q8_0. — Matches llama.cpp master common/download.cpp: re_split, the sidecar table at lines 666-670, the imatrix exclusion, and tags = {"Q4_K_M", "Q8_0"}. My 1,000-repo corpus showed the same categories (mmproj 670, mtp 167, imatrix 101, dspark 32, dflash 22, eagle3 4) and 2,434 shards.
- llama.cpp's downloader uses refs, then tree, then resolve, stores in the HF cache layout, and resumes with .downloadInProgress + .etag and a Range header. — Confirmed in common/hf-cache.cpp (api/models/{repo}/refs, tree/{commit}?recursive=true, lfs.oid as blob name, blobs/ snapshots/ refs/ .locks/, symlink with copy fallback) and common/download.cpp (PARTIAL_SUFFIX, Range: bytes=N-, discard on non-206 or etag mismatch). Master is at 3b4d6ab and the newest release tag is b12242, which the researcher could not read.
- hf-hub 1.0.0 is the current official Rust client, has a non-optional hf-xet dependency, and downloads through the Xet CAS path whenever X-Xet-Hash is present. — crates.io: hf-hub 1.0.0 published 2026-07-10, Apache-2.0, hf-xet ^1.5.3 non-optional, reqwest ^0.13. src/repository/download.rs branches on has_xet_hash to xet_download_*; I found no switch or environment variable to disable Xet. Its lockfile has 356 packages. It also reads HF_TOKEN, HF_ENDPOINT and HF_HOME from the environment.
- Client versions: huggingface_hub 2.4.0, @huggingface/hub 2.17.10, @huggingface/tasks 0.22.5, @huggingface/gguf 0.4.6. — All four match PyPI and npm today.
- Family availability: no official Qwen3.5+ GGUF repos; meta-llama publishes no GGUF; mistralai publishes ungated Ministral 3 GGUFs; Gemma 3 QAT GGUFs are gated manual; Gemma 4 QAT GGUFs are ungated. — Qwen/Qwen3.5-4B-GGUF returns 401 and the newest official Qwen GGUF is Qwen3-Coder-Next (2026-02-04). author=meta-llama&filter=gguf is empty. mistralai lists six Ministral-3 GGUF repos, all gated:false. Google's gemma-3 QAT repos are 'manual' and gemma-4 QAT repos are false. ibm-granite has granite-4.2 3b, 8b and 30b GGUF repos, ungated.
- All 23 seed files exist at the pinned commits with the stated sizes and SHA-256, in ungated repos tagged license:apache-2.0. — I re-checked every repo, commit, filename, size and sha256 through /api/models/{repo}/revision/{commit}?blobs=true: all match, every pinned commit still equals main, all are gated:false with license:apache-2.0. Context lengths and architectures (qwen35, qwen35moe, gemma4, granite, gpt-oss, mistral3) match the brief. Five seed files were also HEAD-checked for a 302.
- Every seed model's embedded chat template maps to a dedicated llama.cpp tool-call handler (static match, not run). — I replicated every rule in common_chat_template_detect_params_init in order (the researcher did four) against all 14 seed templates: Qwen3.5/3.6/3.8 and Granite 4.2 hit Qwen3-Coder XML, Gemma 4 hits the current (non-legacy) gemma4 handler, gpt-oss hits GPT-OSS, Ministral hits ministral_3. All six architectures exist in master's llama-arch.cpp. Still static only; no model was run. Note use_jinja defaults to true in master, so --jinja is no longer strictly needed although the docs still say so.
- There is no official recommended-local-models feed; collections exist but carry no tier, file or hash data. — The cited collection exists (Multimodal GGUFs, 16 items, last updated 2026-07-30) and has no per-file metadata. I found no official feed either, though a negative cannot be fully proven. Bundling a curated JSON is the right call for an offline, no-telemetry app.
