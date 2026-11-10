# huggingface — research

## Summary

The Hugging Face Hub is fully usable for an in-app GGUF browser and downloader over plain HTTPS from Rust; everything below was checked against the live API on 2026-11-10 unless marked otherwise. The model landscape has moved past the families named in the brief, so the seed list uses Qwen3.5/3.6/3.8, Gemma 4, Granite 4.2, gpt-oss-20b and Ministral 3.

Decision-relevant results:
- **Search:** use `filter=gguf`. `library=gguf` is silently ignored and returns the unfiltered trending list.
- **Hashes:** `siblings[].lfs.sha256` (with `?blobs=true`), `lfs.oid` (tree API) and the `X-Linked-ETag` header are the same value, and it is the file's SHA-256 (confirmed by downloading a 13.6 MB file and hashing it).
- **Download:** every LFS file I probed, including 2023-era repos, returns a 302 to a one-hour signed URL on `us.aws.cdn.hf.co`. The CDN honours `Range`, but its own `ETag` is the Xet hash, not the SHA-256.
- **Allowlist:** from this US egress only `huggingface.co` and `us.aws.cdn.hf.co` were needed. Other regional CDN hosts are unverified.
- **Anonymous limits:** 500 API and 3,000 resolve requests per 5 minutes per IP. No User-Agent is required.
- **Curated list:** there is no official source to mirror; ship our own JSON. All 23 seed files were HEAD-verified with size, SHA-256 and pinned commit.

Not verified:
- The 403 response for a valid token that has not accepted a licence (no token available; taken from `huggingface_hub` 2.4.0 source).
- Runtime tool-calling behaviour. No model was run; handler mapping is a static match of each embedded chat template against llama.cpp master.
- llama.cpp release tags (GitHub API unreachable; I read `master` via raw.githubusercontent.com).

One slip: a single probe to huggingface.co sent a User-Agent derived from the working-directory name. Nothing was written to the repository; scratch files are in the session scratchpad only.

## Findings

### Search is GET https://huggingface.co/api/models; use filter=gguf, because library=gguf is silently ignored.

Confidence: high

Live results: `library=gguf&limit=3` returned exactly the same three non-GGUF repos as no filter at all. Unknown parameters are ignored without error. Working parameters: `search` (tokenised match on repo id), `filter` (tags; repeat or comma-separate for AND, e.g. `filter=gguf&filter=license:apache-2.0`, checked on 50 results), `apps=llama.cpp`, `author`, `pipeline_tag` (e.g. image-text-to-text), `gated=true|false`, `num_parameters=min:6B,max:10B`, `sort` (downloads, likes, lastModified, createdAt, trendingScore; default trendingScore; an invalid value gives 400), `limit` (1001 is clamped to 1000), `full=true`, `config=true`, `cardData=true`, `expand[]=`. `direction=1` is rejected with 400 'only descending sort is supported' for downloads and likes. Valid `expand` values, from the server's own 400 message: author, baseModels, cardData, config, createdAt, disabled, downloads, downloadsAllTime, evalResults, gated, inference, inferenceProviderMapping, lastModified, library_name, likes, mask_token, model-index, pipeline_tag, private, safetensors, sha, siblings, spaces, tags, transformersInfo, trendingScore, widgetData, gguf, resourceGroup, xetEnabled. The list endpoint and GET /api/models/{repo} are not in the published OpenAPI spec (3.1.0, 308 paths), so they are a de facto contract only; `huggingface_hub` 2.4.0 `list_models` sends the same parameter names and no `library` or `direction`.

Source: Live curl against huggingface.co/api/models (2026-11-10); https://huggingface.co/.well-known/openapi.json; huggingface_hub 2.4.0 wheel, hf_api.py list_models

### Search response shape depends on the flags; the gguf block gives context length, architecture and chat template, and pagination is by Link header cursor.

Confidence: high

Default fields: `_id, id, modelId, likes, trendingScore, private, downloads, tags, pipeline_tag, library_name, createdAt`. There is no `lastModified`, `gated` or `siblings` unless requested. `full=true` adds `author, gated, lastModified, sha, siblings[{rfilename}]`; `config=true` adds `config`. With `expand[]` only `_id`, `id` and the requested fields come back. `expand[]=gguf` returns `{total, architecture, context_length, chat_template, bos_token, eos_token, quantize_imatrix_file?, totalFileSize}`. `total` is the parameter count, not bytes (27320697856 for a 27B model). `totalFileSize` is the size of whichever single file HF parsed (Q4_0 in one repo, BF16 in another), so per-file sizes must come from siblings. Non-LLM GGUF repos return only `{total, totalFileSize}`. `chat_template` is 4-19 KB per model, so only request `gguf` on detail views or small pages. `gated` is `false`, `"auto"` or `"manual"`. Licence appears as a `license:<id>` tag plus `cardData.license`, `license_name`, `license_link`. Pagination: response header `link: <https://huggingface.co/api/models?...&cursor=<opaque base64>>; rel="next"`; there is no total count; stop when the header is absent. The next URL rewrites `expand[]=x` as repeated `expand=x`, and both forms are accepted.

Source: Live curl with full=true, config=true and expand[] variants; second page fetched via the Link cursor

### Repo detail with ?blobs=true gives per-file size and SHA-256; the tree API gives the same hash under a different key.

Confidence: high

GET /api/models/{repo}?blobs=true returns siblings as `{rfilename, blobId, size, lfs:{sha256, size, pointerSize}}`. `blobId` is the git blob SHA-1; `lfs` is present only for LFS files. Without `blobs=true`, siblings contain only `rfilename`. Top-level keys include `sha` (the commit), `gated`, `private`, `disabled`, `tags`, `cardData`, `config`, `gguf`, `lastModified`, `createdAt`, `usedStorage`. GET /api/models/{repo}/revision/{commit}?blobs=true works the same for a pinned commit. GET /api/models/{repo}/tree/{rev}?recursive=true returns `[{type: file|directory, oid, size, path, lfs:{oid, size, pointerSize}, xetHash}]`; `expand=true` adds `lastCommit` and `securityFileStatus`. Tree `limit` defaults to 1000 (100 with expand) and pages with `cursor` and a Link header. Proof that the hash is the content SHA-256: I downloaded `imatrix_unsloth.gguf` (13,642,656 bytes) and `sha256sum` gave 0ee5b10bd0c2fa2127c6f4b43dbfe1efd71e383b63217af9dade1de36599f1c1, equal to `lfs.sha256`, `lfs.oid` and `X-Linked-ETag`. Pitfall: on gated repos the anonymous tree API masks `lfs.oid` and `xetHash` with asterisks, while `?blobs=true` still returns the real sha256 (seen on google/gemma-3-4b-it-qat-q4_0-gguf). GET /api/models/{repo}/refs returns `{branches:[{name, ref, targetCommit}], tags, converts}`.

Source: Live curl on unsloth/Qwen3.8-27B-GGUF and google/gemma-3-4b-it-qat-q4_0-gguf; local sha256sum; OpenAPI spec entry for the tree endpoint

### Downloads go through /resolve/{rev}/{file}: LFS files 302 to a one-hour signed URL on us.aws.cdn.hf.co, and the CDN supports Range resume.

Confidence: high

HEAD or GET https://huggingface.co/{repo}/resolve/{rev}/{file} for an LFS file returns 302 with `location: https://us.aws.cdn.hf.co/xet-bridge-us/{repo _id}/{xet hash}?...&Expires=...&Policy=...&Signature=...&Key-Pair-Id=...`. `Expires` was exactly 3600 s ahead. Headers on the 302: `x-repo-commit` (40-hex commit), `x-linked-size` (bytes), `x-linked-etag` (quoted SHA-256), `x-xet-hash`, `accept-ranges: bytes`, a `link` header with rel=xet-auth and rel=xet-reconstruction-info, and `x-hf-warning: unauthenticated; ...`. There is no plain `etag` on the 302. On the CDN: `Range: bytes=0-1023` gave 206 with `content-range: bytes 0-1023/13642656`; an open-ended mid-file range gave 206; a range starting at the file size gave 416; HEAD gave 200 with `content-length`. The CDN `etag` is the Xet hash, not the SHA-256, so resume validation should use `x-linked-etag` or `x-repo-commit` from huggingface.co. Small non-LFS files (README, config.json) return 307 to a relative `/api/resolve-cache/models/{repo}/{commit}/{file}` on the same host, with `x-linked-etag` equal to the git blob SHA-1. `{rev}` may be a branch or a full commit sha. A 2023 TheBloke repo, ggml-org, Qwen, bartowski and unsloth repos all redirected to the same CDN host and path prefix.

Source: Live curl -I and ranged GETs against huggingface.co and us.aws.cdn.hf.co; OpenAPI entry 'Resolve a file'

### The redirect crosses hosts, and reqwest 0.13.5 strips Authorization on that hop by default; only two hosts were needed from a US egress.

Confidence: medium

Hosts actually contacted: `huggingface.co` (API, resolve, resolve-cache) and `us.aws.cdn.hf.co`. I could not test other regions. A community-compiled forum list also names `us.gcp.cdn.hf.co`, `cas-bridge.xethub.hf.co`, `cdn-lfs.hf.co`, `cdn-lfs-us-1.hf.co` and `cdn-lfs-eu-1.hf.co`, and there is no official allowlist page. `cas-server.xethub.hf.co` is only needed for the native Xet protocol, which plain HTTP download does not use. reqwest 0.13.5 `redirect.rs::remove_sensitive_headers` removes Authorization, Cookie, cookie2, Proxy-Authorization and WWW-Authenticate when host, port or scheme changes; the default policy is `Policy::limited(10)`. `huggingface_hub` 2.4.0 does the same explicitly: it stops following at the first off-Hub redirect and pops `authorization` before fetching the CDN URL. The CDN returned 206 even with a bogus Authorization header attached, so a leaked token would not cause a visible failure.

Source: Live curl; ~/.cargo/registry reqwest-0.13.5/src/redirect.rs; huggingface_hub 2.4.0 utils/_http.py and file_download.py; https://discuss.huggingface.co/t/how-to-get-a-list-of-all-huggingface-download-redirections-to-whitelist/30486

### Auth is `Authorization: Bearer hf_...`; gated files return 401 with X-Error-Code: GatedRepo anonymously, and errors are signalled in X-Error-Code / X-Error-Message headers.

Confidence: medium

Anonymous GET of a gated LFS file: 401, `x-error-code: GatedRepo`, `x-error-message: Access to model <repo> is restricted. You must have access to it and be authenticated to access it. Please log in.`, `www-authenticate: Bearer realm="Authentication required"`, body is the same text as text/plain. The response is identical for `"manual"` and `"auto"` gated repos and with an invalid token. With a valid token that has not been granted access the status is 403 with the same GatedRepo code and the message '...is restricted and you are not in the authorized list. Visit https://huggingface.co/<repo> to ask for access.' (from the `huggingface_hub` GatedRepoError docstring; not live-tested). Gated repo metadata is readable anonymously: model info, sibling list with sizes and sha256, and README.md via resolve all returned 200. Other cases: nonexistent or private repo gives 401 `{"error":"Invalid username or password."}` with no X-Error-Code, so it cannot be told apart from a private repo; missing file gives 404 `EntryNotFound`; bad revision gives 404 `RevisionNotFound`. An invalid or garbage token on public endpoints is silently ignored (200 on the API, 302 on resolve); only GET /api/whoami-v2 returned 401 for it. HF docs state access requests can only be made from the browser.

Source: Live curl on google/gemma-3-4b-it-qat-q4_0-gguf and orcarouter/Qwen3.8-27B-Uncensored-GGUF; huggingface_hub 2.4.0 errors.py and utils/_http.py; https://huggingface.co/docs/hub/models-gated

### Anonymous rate limits are 500 API and 3,000 resolve requests per 5-minute fixed window per IP, reported on every response; no User-Agent is required.

Confidence: high

Observed headers: `ratelimit: "api";r=366;t=29` with `ratelimit-policy: "fixed window";"api";q=500;w=300`, and `ratelimit: "resolvers";r=2929;t=148` with `q=3000;w=300`. `r` is remaining requests and `t` is seconds to reset. The docs table (dated September 2025, 'subject to change') gives Anonymous 500 / 3,000 / 100 (API / Resolvers / Pages) and Free user 1,000 / 5,000 / 200. Exceeding the limit gives 429. The tree and model-info endpoints count against the API bucket; anything with `/resolve/` in the path counts against Resolvers. Requests with an empty or absent User-Agent returned 200 and 302. Conventions in official clients: llama.cpp sends `llama-cpp/<build info>`; the Rust hf-hub crate sends `hf-hub/1.0.0`; Python sends `unknown/None; hf_hub/2.4.0; python/3.x` with `; `-separated `key/value` extras.

Source: Live response headers; https://huggingface.co/docs/hub/rate-limits; llama.cpp master common/download.cpp and common/hf-cache.cpp; hf-hub 1.0.0 src/client.rs; huggingface_hub 2.4.0 utils/_headers.py

### Quantization is the last encoding token of the filename; a Rust-compatible regex matched 96.9% of main model files in the 2,000 most-downloaded GGUF repos.

Confidence: high

The GGUF spec convention is `[<Sidecar>-]<BaseName>-<SizeLabel>-<FineTune>-<Version>-<Encoding>-<Type>-<Shard>.gguf` and says it is 'not intended to be perfectly parsable'; its own validation regex requires a `v1.0` version token that almost no Hub file has. Practical regex (no lookaround, works in the `regex` crate): `(?i)(?:^|[-._/])((?:UD-)?(?:(?:IQ[1-4]|T?Q[1-8])_(?:K_(?:XXL|XL|L|M|S|P)|K|XXS|XS|NL|S|M|L|[0-9](?:_[0-9]_[0-9])?)|BF16|F16|F32|FP16|FP32|MXFP4(?:_MOE)?|NVFP4|F8_E4M3))(?:[-._/]|$)`. Apply it to the basename with `.gguf` and any shard suffix removed, take the last match, and fall back to the full path so `Q4_K_M/model-00001-of-00003.gguf` resolves. Corpus: 34,052 .gguf files; excluding two non-model repos (control vectors, adapters), 96.93% of 30,221 main files matched. Misses are bare labels such as `q8`, `q4km`, `Balanced`, `4.94bpw`. Most common labels: Q8_0, Q4_K_M, Q6_K, Q5_K_M, BF16, Q3_K_M, Q4_K_S. `UD-` marks Unsloth Dynamic quants (UD-Q4_K_XL etc.); gpt-oss ships as `MXFP4`; Google QAT files use lowercase `q4_0`, sometimes mid-name (`gemma-4-E4B_q4_0-it.gguf`). HF's own parser (`@huggingface/tasks` 0.22.5 `GGUF_QUANT_RE`) is `(UD-)?(<enum names>|MXFP4)(_[A-Z]+)?` on the uppercased name, last match wins. llama.cpp's tag regex is `[-.]([A-Z0-9_]+)$`, and its default pick when no tag is given is Q4_K_M, then Q8_0, then the first model file.

Source: https://raw.githubusercontent.com/ggml-org/ggml/master/docs/gguf.md; @huggingface/tasks 0.22.5 src/gguf.ts; llama.cpp master common/download.cpp; corpus from /api/models?filter=gguf&sort=downloads&limit=1000&expand[]=siblings (two pages)

### Split files use a 5-digit `-NNNNN-of-NNNNN.gguf` suffix, and sibling lists contain several kinds of non-model GGUF that must be filtered out.

Confidence: high

Split regex, as in llama.cpp: `(?i)^(.+)-([0-9]{5})-of-([0-9]{5})[.]gguf$`. 6,933 of 34,052 corpus files (20%) were shards. Shards share a prefix, are often in a per-quant subdirectory (`BF16/Qwen3.8-27B-BF16-00001-of-00002.gguf`), and sizes must be summed. The first shard can be metadata-only (10.9 MB next to a 102 GB second shard in ggml-org/Qwen3.8-Flash-Next-GGUF). llama.cpp is given shard 00001 and needs all shards in the same directory. Sidecar and auxiliary files, mirroring llama.cpp `gguf_filename_is_model`: any basename containing `mmproj`; basenames starting `mtp-`, `dflash-`, `eagle3-`, `dspark-` (speculative-decoding draft modules); any basename containing `imatrix`. Corpus counts: mmproj 1,212, imatrix 241, mtp 210, dspark 36, dflash 33, eagle3 4. mmproj is not always a prefix (235 of 1,212): `Ministral-3-14B-Instruct-2512-BF16-mmproj.gguf`, `gemma-4-E4B-it-mmproj.gguf`, `Simplicio-27B.mmproj-Q8_0.gguf`; also `mmproj-F16.gguf`, `mmproj-<model>-Q8_0.gguf`, `mmproj-model-f16.gguf`. A repo may offer several mmproj precisions (F32, BF16, F16, Q8_0); llama.cpp picks the one in the deepest shared directory with the closest bit width to the model. 11,756 main files sat in subdirectories, so always keep the full `rfilename` path.

Source: llama.cpp master common/download.cpp (get_gguf_split_info, sidecars table, find_best_sibling); filename corpus of 2,000 repos; ?blobs=true listings

### llama.cpp's own downloader is a usable reference design: refs, then tree, then resolve, stored in the standard Hugging Face cache layout.

Confidence: high

`common/hf-cache.cpp` calls GET `api/models/{repo}/refs` to get the main commit, then GET `api/models/{repo}/tree/{commit}?recursive=true`, and uses `lfs.oid` (or `oid` for non-LFS files) as the blob name. It writes `blobs/<oid>`, `snapshots/<commit>/<path>` (symlink, with copy fallback) and `refs/`, under LLAMA_CACHE, HF_HUB_CACHE, HF_HOME/hub or ~/.cache/huggingface/hub, and shares `.locks/` with `huggingface_hub`. `common/download.cpp` keeps a partial file as `<path>.downloadInProgress` plus `<path>.etag`, resumes with `Range: bytes=<downloaded>-` when `Accept-Ranges` is not `none`, and discards the partial on a non-206 reply or an ETag mismatch. It does a single tree request with no cursor handling, which is fine below 1,000 entries. A repo-root `preset.ini` short-circuits file selection.

Source: https://raw.githubusercontent.com/ggml-org/llama.cpp/master/common/hf-cache.cpp and common/download.cpp (master, fetched 2026-11-10)

### The official Rust client hf-hub 1.0.0 exists but pulls in the Xet protocol; hand-rolled reqwest 0.13.5 is the simpler fit for a strict allowlist.

Confidence: high

crates.io: `hf-hub = "1.0.0"` (published 2026-07-10, Apache-2.0, github.com/huggingface/hf-hub) and `reqwest = "0.13.5"`. hf-hub 1.0.0 has a non-optional dependency on `hf-xet ^1.5.3` (plus reqwest ^0.13, sha2 ^0.11, tokio). `src/repository/download.rs` HEADs the resolve URL and, when `X-Xet-Hash` is present (it was on every LFS file I probed), downloads through the Xet CAS path rather than the plain HTTP bridge, which adds hosts such as `cas-server.xethub.hf.co`. It builds a second client with `redirect::Policy::none()` for metadata calls. Other current client versions for reference: `huggingface_hub` 2.4.0 (PyPI), `@huggingface/hub` 2.17.10, `@huggingface/tasks` 0.22.5, `@huggingface/gguf` 0.4.6.

Source: cargo search / cargo info (cargo 1.97.0); crates.io dependency API for hf-hub 1.0.0; hf-hub-1.0.0 source in ~/.cargo/registry; npm view; PyPI JSON

### There is no official, stable 'recommended local models' feed to mirror; the app must ship its own curated JSON.

Confidence: high

What exists: the dynamic filters above (`apps=llama.cpp`, `sort=trendingScore` or `downloads`, `num_parameters`) and editorial collections via GET /api/collections/{slug} (for example `ggml-org/multimodal-ggufs-68244e01ff1f39e5bebeeedc`, 16 items, last updated 2026-07-30). Collections carry no memory tier, file choice, sha256 or capability data, and their item ids mix repo ids with nested collection ids. Trending and download rankings are dominated by uncensored finetunes and non-LLM GGUFs (audio, image, video), so they are unsuitable as recommendations without filtering. Suggested schema, keyed by tier with a shared models map: `{schema_version: 1, generated: '2026-11-10', tiers: {'8': {default: <id>, models: [<id>...]}, '12': {...}, '16': {...}, '24': {...}, '32': {...}}, models: {<id>: {name, repo, revision (40-hex commit), files: [{path, size, sha256}] (ordered, so split GGUFs work), mmproj: {path, size, sha256} | null, quant, params, architecture, context_length, recommended_ctx, capabilities: ['tools','vision','thinking'], tool_format, license: {spdx, name?, url?}, gated: false|'auto'|'manual', base_model}}}`. Pinning `revision` keeps sha256 and size valid, because /resolve/{commit}/{file} is immutable.

Source: Live /api/collections and /api/models probes; HEAD of resolve by commit sha

### The families named in the brief are mostly superseded or awkward; current ungated Apache-2.0-tagged options are Qwen3.5/3.6/3.8, Gemma 4, Granite 4.2, gpt-oss-20b and Ministral 3.

Confidence: high

- Qwen: there are no official `Qwen/Qwen3.5-*-GGUF` repos (401 on Qwen/Qwen3.5-4B-GGUF); GGUFs come from unsloth, ggml-org (the llama.cpp org), lmstudio-community and bartowski. Qwen3.5+ repos are tagged image-text-to-text and ship an mmproj, so they replace the separate Qwen3-VL line. Qwen3-VL (Oct 2025) still exists in the official org.
- Gemma: Google's Gemma 3 QAT GGUF repos are gated `manual`; the Gemma 4 QAT repos are `gated: false` and tagged `license:apache-2.0` (with a license_link to ai.google.dev/gemma/docs/gemma_4_license).
- Llama: `meta-llama` publishes no GGUF repos and its source repos are gated `manual`.
- Mistral: `mistralai` publishes official Ministral 3 GGUFs (3B/8B/14B, Apache-2.0). Mistral Small 3.2 24B is available via unsloth (Apache-2.0). Mistral Small 4 is 119B, and Devstral Small 2 is tagged `license:other`.
- Excluded: Qwen3.8-Flash-Next (177B parameters, `license:other`).
- Context lengths from the gguf block: Qwen3.5/3.6/3.8 262144; Gemma 4 E4B 131072, 12B/26B/31B 262144; Granite 4.2 131072; gpt-oss-20b 131072; Ministral 3 262144.

Source: Live /api/models listings by author and search, and /api/models/{repo}?blobs=true for each repo

### Seed models for the 8, 12 and 16 GB tiers (every file HEAD-verified: 302, size and SHA-256 match; format is repo@commit, file, bytes, sha256).

Confidence: high

8 GB
- unsloth/Qwen3.5-4B-GGUF@e87f176479d0855a907a41277aca2f8ee7a09523 (tools + vision): `Qwen3.5-4B-Q4_K_M.gguf` 2740937888 00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4; `mmproj-F16.gguf` 672423616 cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864
- ibm-granite/granite-4.2-3b-GGUF@c40945d71cd90f249a56985e8155551a9188dc30 (tools, text only): `granite-4.2-3b-Q4_K_M.gguf` 2244011552 e0406663965846ae22a403456eb826ccce5f450840491f71952f18a7cb78e7d5
- ggml-org/gemma-4-E4B-it-GGUF@27cd6836f8e2a6e5e6b57ea0e77fa72490706985 (tools + vision, 5.15 GB total, tight): `gemma-4-E4B-it-Q4_0.gguf` 4590807392 a555b900214b477d8880e7832e0b8925e139b0159640036b09fe472b6f2097f2; `mmproj-gemma-4-E4B-it-Q8_0.gguf` 559874848 9257f21a0f5358204b4a612cff83fad2e56412f4f607f0049331ce60c0cf9d34

12 GB
- unsloth/Qwen3.5-9B-GGUF@3885219b6810b007914f3a7950a8d1b469d598a5 (tools + vision): `Qwen3.5-9B-Q4_K_M.gguf` 5680522464 03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8; `mmproj-F16.gguf` 918166080 f70dc3509053962b0d0d3ee8a7eacebf5d60aa560cad78254ae8698516ae029f
- google/gemma-4-12B-it-qat-q4_0-gguf@29d097773436b69ff9feafd636ab4cf873786537 (tools + vision): `gemma-4-12b-it-qat-q4_0.gguf` 6975879296 93567e57a8fe10b23569b9d9ec38cd005deedf71e29477c421a4b83f418a538b; `mmproj-gemma-4-12b-it-qat-q4_0.gguf` 175115616 cb018338a7538a9814d994bfe54644c71eb7ed54e31eae2f721e45fd3c260da7
- ibm-granite/granite-4.2-8b-GGUF@93f3f6a8938ee922b784cf4e5b4203cd3428df8f (tools, text only): `granite-4.2-8b-Q4_K_M.gguf` 5347917952 16a9369d0805f80b7377d25d87f937a90c05dc04ad79173a52001e42c9aab311

16 GB
- ggml-org/gpt-oss-20b-GGUF@15dcfc7e7c608c6161507211ed43c5b590801b4b (tools, text only): `gpt-oss-20b-MXFP4.gguf` 12109566624 27cd6c432c7672cb812a92f611cf3ba7bbc35928262bb1e1253ff4ee6ae35901
- mistralai/Ministral-3-14B-Instruct-2512-GGUF@74fac473c43357d7fb2671713608183cc72496d0 (tools + vision): `Ministral-3-14B-Instruct-2512-Q4_K_M.gguf` 8239593024 824e0f3373e69b84f2cae46fdcb9bd1ebc6ab3bfc7acc125d818b7b8178cc613; `Ministral-3-14B-Instruct-2512-BF16-mmproj.gguf` 879258784 ab7616148b7081030fd38e410c9213ce8346a8b0588291c8ca279058e78cfae7

All are `gated: false` and tagged `license:apache-2.0`. Tier placement is by file size only; no model was loaded.

Source: /api/models/{repo}?blobs=true plus HEAD on /resolve/main/{file} for each file (x-linked-size, x-linked-etag, x-repo-commit), 2026-11-10

### Seed models for the 24 and 32+ GB tiers (every file HEAD-verified; same format).

Confidence: high

24 GB
- unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502 (tools + vision, 17.4 GB total): `Qwen3.8-27B-UD-Q4_K_M.gguf` 16464440224 322e194ff79741c7baa497c240f677f54b201b0efab44ca8e50f122b39123482; `mmproj-F16.gguf` 927607488 cbb841a9ee0636b2ec172f5bb8df2ea8dfeb01e90fe7c6126581d662a0b4e43e
- google/gemma-4-26B-A4B-it-qat-q4_0-gguf@d1c082be9cf3c8a514acf63b8761f4b41935842e (tools + vision): `gemma-4-26B_q4_0-it.gguf` 14439363584 3eca3b8f6d7baf218a7dd6bba5fb59a56ee25fe2d567b6f5f589b4f697eca51d; `gemma-4-26B-it-mmproj.gguf` 1194828160 a359953a076b877db30c31dbbb4c6d93b4a6e017ee5db5784247e4d4c0dd4f3b
- ibm-granite/granite-4.2-30b-GGUF@27b350a791e81d9a4d1ddca1c49282e9ec533768 (tools, text only): `granite-4.2-30b-Q4_K_M.gguf` 17721455328 f299dace85d77ee0e24ca0b6720bd211b1f4cd1f65b9c0a2111e79692e29c9be

32+ GB
- ggml-org/Qwen3.6-35B-A3B-GGUF@eb0b3cc97fecddab7eee6e6ffdbe92d2a00afcea (tools + vision, MoE): `Qwen3.6-35B-A3B-Q4_K_M.gguf` 20542732576 f4ffe03c9139e6a905a5d346f77d9fabdd179367c3280dfd005d1e75ca5ed8c8; `mmproj-Qwen3.6-35B-A3B-Q8_0.gguf` 614194304 904cbf8c8e876220066ab3bf676c7efa40f3da372276fdaf8b01d2fb2a37a51d
- unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502 (higher precision): `Qwen3.8-27B-UD-Q6_K.gguf` 21983677344 c9c206812fbe4ac7b76a729e25928b63f2ae89d37f69da7a71c20aec763cd436, with the same `mmproj-F16.gguf` as above
- google/gemma-4-31B-it-qat-q4_0-gguf@59dde24573e7e61570dba08b18a2e1fe246955ed (tools + vision): `gemma-4-31B_q4_0-it.gguf` 17651001568 179cfb99212709597eae5929112cfca677e1bbf566178b479ae1da0c4772874b; `gemma-4-31B-it-mmproj.gguf` 1200726368 6bd60bdb958548b4093196d38744b0f2290c12503a3fddd7486bffa9c5eb07a4

Alternative provenance: ggml-org/Qwen3.8-27B-GGUF has `Qwen3.8-27B-Q4_K_M.gguf` at 19403684480 bytes (d15d4b6de55f662ac40e867702d31afe738d032fa06b829b2b1a21a49d4a48ee), which with its mmproj is about 20 GB and tight for 24 GB. All are `gated: false` and tagged `license:apache-2.0`.

Source: /api/models/{repo}?blobs=true plus HEAD on /resolve/main/{file} for each file, 2026-11-10

### Every seed model embeds a tools-aware chat template, and by static match each maps to a dedicated llama.cpp tool-call handler; this was not run.

Confidence: medium

I applied the first-match rules in llama.cpp master `common/chat.cpp` (`common_chat_template_detect_params_init`) to each repo's embedded `gguf.chat_template`. I replicated only the Ministral, GPT-OSS, Gemma 4 and Qwen3-Coder checks, not every intermediate rule, so treat this as indicative. Results: Qwen3.5, Qwen3.6, Qwen3.8 and Granite 4.2 templates contain `<tool_call>`, `<function=` and `<parameter=`, matching the 'Qwen3-Coder XML' handler (whose source comment names Qwen3.5). Gemma 4 templates (Google QAT and ggml-org) contain `'<|tool_call>call:'` and the `{#- OpenAI Chat Completions:` marker, matching the current Gemma 4 handler rather than the legacy path that logs 'detected an outdated gemma4 chat template'. gpt-oss matches the GPT-OSS (Harmony) handler via `<|channel|>`. Ministral 3 matches the dedicated Ministral handler. Templates that fall to the generic differential autoparser: Qwen3-VL and Qwen3 (Hermes-style JSON), Mistral Small 3.2, granite-vision-4.1. llama.cpp's docs still say tool calling needs `llama-server --jinja`, list GPT-OSS and granite-4.1-3b under native support, and warn that extreme KV-cache quantization (`-ctk q4_0`) degrades tool calling. All Qwen3.5+, Gemma 4 and Granite 4.2 templates reference `enable_thinking`.

Source: https://raw.githubusercontent.com/ggml-org/llama.cpp/master/common/chat.cpp, common/chat.h, docs/function-calling.md; gguf.chat_template from /api/models/{repo}

## Recommendations

- Build the browser on `GET /api/models?filter=gguf&search=...&sort=downloads|trendingScore|lastModified&limit=50` with `expand[]=downloads&expand[]=likes&expand[]=gated&expand[]=tags&expand[]=lastModified&expand[]=pipeline_tag`. Never send `library=`. Leave `gguf` and `siblings` to the detail call so list pages stay small.
- Page by following the `Link: <...>; rel="next"` URL verbatim until it disappears; there is no total count, so use infinite scroll rather than page numbers.
- For the detail view call `GET /api/models/{repo}?blobs=true` once. It returns the commit `sha`, `gated`, licence tags and cardData, the `gguf` block, and every file's size and `lfs.sha256`. Use the tree API only for repos with more than about 1,000 files or when listing a pinned commit.
- Download with two reqwest clients and manual redirects. Client A (`redirect::Policy::none()`, optional Bearer token) calls `https://huggingface.co/{repo}/resolve/{commit}/{file}` and reads `x-linked-size`, `x-linked-etag`, `x-repo-commit` and `Location`. Client B (no auth headers) fetches the Location only if its host passes the allowlist. Resolve same-host relative 307s with client A.
- Start the network allowlist at `huggingface.co` and `us.aws.cdn.hf.co`, make it data-driven, and show a clear 'blocked redirect host: X' error. Test from an EU network before release, since other regional CDN hosts are unverified.
- Resume with `Range: bytes=<have>-` and require a 206 whose `Content-Range` total equals `x-linked-size`. Always re-resolve through huggingface.co before each resume or retry, because the signed CDN URL expires after one hour. Treat 416 as 'already complete, go verify'.
- Key partial files on the SHA-256 from `x-linked-etag` (quotes stripped), not on the CDN ETag. Stream a SHA-256 over the whole file at the end and compare with `lfs.sha256` before atomically renaming; discard the partial if the sha256 or commit has changed.
- Pin a 40-hex commit as `{rev}` for curated models, and record the resolved `x-repo-commit` for user-chosen models.
- Map errors from `X-Error-Code` and status: 401 + GatedRepo means 'sign in with a token'; 403 + GatedRepo means 'open the model page in the browser and accept the licence'; 401 without a code means 'repo not found or private'; 404 EntryNotFound / RevisionNotFound; 429 means wait `t` seconds from the `RateLimit` header. Show the `gated` badge from metadata before the user clicks download.
- Validate a pasted token with `GET /api/whoami-v2`, because invalid tokens are silently ignored on public endpoints. Keep the token in the OS keychain and attach it only to huggingface.co requests.
- Send a stable, non-identifying `User-Agent: <app>/<version>`, debounce search-as-you-type, cache list and detail responses briefly, and back off on 429 using the `RateLimit` header.
- Classify sibling files in this order: not `.gguf` is ignored; basename contains `mmproj` is a projector; basename starts with `mtp-`, `dflash-`, `eagle3-` or `dspark-`, or contains `imatrix`, is auxiliary and hidden; then group shards by the split regex and sum their sizes; then extract the quant label with the regex, falling back to 'unknown' and showing the raw filename.
- Download all shards of a split model into one directory and pass shard 00001 to llama-server. Pair the mmproj from the same repo and directory, preferring F16 or Q8_0 over F32.
- Ship the curated list as a bundled JSON using the proposed schema with pinned `revision`, per-file `sha256` and `size`. Refresh it per app release with a script that re-runs the `?blobs=true` and HEAD checks.
- Suggested tier defaults: 8 GB Qwen3.5-4B Q4_K_M; 12 GB Qwen3.5-9B Q4_K_M; 16 GB gpt-oss-20b MXFP4 for text and tools, or Ministral-3-14B Q4_K_M when vision is needed; 24 GB Qwen3.8-27B UD-Q4_K_M; 32+ GB Qwen3.6-35B-A3B Q4_K_M. Offer the Gemma 4 QAT and Granite 4.2 entries as alternatives.
- Before shipping the seed list, run each model once under the bundled llama-server with `--jinja` and a real tool-call round trip (including a parallel call and one image for vision models), and record the result and the llama.cpp build in the JSON.
- Do not adopt the hf-hub 1.0.0 crate for downloads unless the Xet protocol and its extra hosts are wanted; reuse its ideas only (separate no-redirect client, HF cache layout).

## Risks

- The list and detail endpoints (`/api/models`, `/api/models/{repo}`) are not in the published OpenAPI spec and unknown parameters are silently ignored, so a renamed parameter would degrade results without an error. Add a contract test that asserts every returned repo has the `gguf` tag.
- The CDN hostname is not contractual. It has already changed over time (cdn-lfs*.hf.co, then cas-bridge.xethub.hf.co, now us.aws.cdn.hf.co) and may differ by region or cloud. A hard-coded allowlist can break downloads without any app change.
- Signed CDN URLs last one hour. A large GGUF on a slow link will outlive the URL, and any reconnect to the stale URL will fail unless the app re-resolves.
- Anonymous limits are per IP and documented as 'subject to change'. Users behind shared NAT or VPN can hit 429 on the 500-per-5-minute API bucket, especially with search-as-you-type.
- The 403 behaviour for a token that has not accepted a gated licence was taken from client source, not observed. The exact status and message should be confirmed with a real token.
- Tool-calling reliability is inferred from template text and llama.cpp master source, not from running the models. The handler that matches depends on the llama.cpp build bundled with the app, and older builds may fall back to generic parsing or lack the `qwen35`, `gemma4` or `granite` architectures entirely.
- Tier placement is by file size only. Real memory use adds KV cache and compute buffers that grow with context (these models advertise 131k-262k context), and on 8 GB unified-memory Macs usable memory is well below 8 GB.
- Unpinned `main` references drift: quantizers re-upload files (ggml-org repos were modified on 2026-11-08), which changes sha256 and size. Seed entries must use the pinned commits.
- Third-party quantizer repos (unsloth, lmstudio-community, bartowski) can be renamed, gated or deleted. Only ggml-org, google, ibm-granite and mistralai entries are first-party or llama.cpp-maintainer provenance.
- Licence tags are self-declared repo metadata. Gemma 4 is tagged apache-2.0 but links to a separate Gemma 4 licence page, so the actual terms should be read before describing it as Apache-2.0 in the UI.
- The gated-repo tree API masks hashes for anonymous callers while `?blobs=true` does not. This inconsistency may be closed in either direction, so code should tolerate missing or masked sha256 on gated repos until authenticated.
- Trending and download rankings surface many uncensored finetunes and non-LLM GGUFs (audio, image, video). An unfiltered browser will show models that llama-server cannot load or that the product may not want to promote.
- The quant regex leaves about 3% of real filenames unlabelled, and new schemes appear regularly (NVFP4, MXFP4, UD- variants, mtp/dflash/eagle3/dspark sidecars all postdate older parsers). The sidecar and quant tables need to be updatable without a code release.
