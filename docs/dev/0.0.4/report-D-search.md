# Report D: web search with sources (interim, 2026-11-11 19:55 UTC, saved by the lead)

## Finished, built, tested (Rust)
- `crates/search` (new): providers (duckduckgo, brave, tavily, searxng; parsers tested
  against fixtures), guarded fetcher, text extraction, BM25 passages, project library
  versions, official hosts and version notes, the pipeline. `cargo +1.99 test -p search`:
  87 unit, 15 fetcher integration and 11 pipeline integration tests passed; 4 live tests
  are ignored by default. clippy clean with and without the test hooks.
- `crates/policy/src/engine.rs`: `Scope::WebSearch` and the test
  `web_search_is_grantable_and_asks_again_after_untrusted_content`.
- `crates/store/src/sources.rs` (new), one line in the `[sources]` schema block
  (`CREATE UNIQUE INDEX sources_chat_url ON sources(chat_id, url)`). 7 tests.
- `src-tauri/src/search_cmd.rs` and `search_cmd/tests.rs` (new): server `search`
  (`web_search`, `read_source`); commands `get_search_settings`, `set_search_settings`,
  `set_search_key`, `clear_search_key`, `test_search`, `list_sources`; event
  `sources-changed`. 20 tests. Registered in `lib.rs`, `commands.list`,
  `capabilities/main.json`, `toolset.rs`. Workspace dependencies `dom_query`,
  `html5ever`, `toml`, `encoding_rs` were already in `Cargo.lock` (no new package).

## Not finished at the time of this note (frontend)
- Written: `src/components/sources/types.ts`, `api.ts`, `state.ts`.
- Written and FAILING until their modules exist: `citations.test.ts`, `quotes.test.ts`.
- Not started: citation rendering in `Markdown.tsx` and the prop in `MessageItem.tsx`,
  `SourcesTab.tsx` (still the placeholder), the tool card extra for `web_search`,
  `SearchSettings.tsx` and its mount, the e2e scenario `search`,
  `python3 scripts/notices.py`, the npm checks.
- Without the frontend: the model can search, read sources and cite `[n]`; sources are
  stored per chat; nothing in the window shows them, and the provider stays DuckDuckGo
  unless `set_search_settings` is invoked.

## Security properties that hold (tested)
The fetcher refuses loopback, private, link-local, CGNAT, unique-local, unspecified,
multicast and reserved addresses (IPv4, IPv6, IPv4-in-IPv6, decimal, hex and octal
spellings, localhost names) before connecting. A name with one blocked answer is
refused. Each hop gets its own HTTP client whose only resolver knows the checked
addresses (no second lookup), no proxy, no client-side redirects. Every redirect hop is
re-checked (test: a public-looking host answers 302 to loopback, private and metadata
addresses; the trap server sees no connection). http and https only, no credentials in
URLs, 10 seconds for the whole fetch, 2 MB body, text/html and text/plain only,
User-Agent `<app name>/<version>`, no cookies, referrer or language. With the guard
switched off by hand, 4 fetcher tests fail. The address classification is a copy of the
browser's `netpolicy`; an app-crate test compares both over more than 10,000 addresses.
API keys live only in the keychain (`search.brave`, `search.tavily`); DTOs carry only
`braveKeySet` and `tavilyKeySet`. Retrieved text is sanitised with `policy::sanitize`
and capped; stored text is exactly what the model was given.

## Policy
Both tools: scope `WebSearch`, outbound and network, read-only. Because of D9, after
any tool result in a turn the next `web_search` or `read_source` asks again, grant or
not. (The lead intends to change this at integration: see the step 1 status.)

## Live search
Partly tried. The guarded fetcher works against real hosts from the build environment
(npm, PyPI and crates.io registry answers and a page were fetched and parsed).
DuckDuckGo could NOT be verified: from this environment `html.duckduckgo.com` answers
with its bot challenge (HTTP 202) or resets the connection. The app reports that as
"DuckDuckGo refused the search…" (the challenge fixture matches the real page), but the
results parser is tested only against a hand-written fixture. Brave and Tavily are
unverified (no keys). SearXNG is verified only against a local stand-in.

## The result as the model sees it
Pinned by the test `the_result_of_a_search_as_the_model_sees_it` in
`src-tauri/src/search_cmd/tests.rs`: first line `Sources [1] [2] … for: <query>`, then
per hit `[n] Title — site (official documentation; matches tokio 1.38)`, the URL and
the excerpt, and last the citing instruction.

## For others
The Tools panel shows the group under its raw name `search` (the label map is in
`ToolsPanel.tsx`).
