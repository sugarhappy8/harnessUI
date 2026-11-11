<!-- Saved by the lead from the developer's final message; the developer could not write this file itself. -->

# Report D: web search with verifiable sources, version-aware (owner items 2 and 10)

Feature D is built end to end and every check I ran passes. Nothing is stubbed. Not verified: a real search through DuckDuckGo (it turned this environment away), Brave and Tavily (no keys), and anything on Windows or macOS.

The file /home/user/harnissUI/docs/dev/0.0.4/report-D-search.md still holds the interim text you saved at 19:55. The harness refused my write to it ("subagents return findings as text"), so please replace it with this report.

## 1. Files created and changed

Created (all under /home/user/harnissUI):
- /home/user/harnissUI/crates/search/Cargo.toml
- /home/user/harnissUI/crates/search/src/lib.rs, netguard.rs, fetch.rs, extract.rs, rank.rs, versions.rs, official.rs, pipeline.rs (same folder)
- /home/user/harnissUI/crates/search/src/provider/mod.rs, duckduckgo.rs, brave.rs, tavily.rs, searxng.rs
- /home/user/harnissUI/crates/search/tests/fetch.rs, pipeline.rs, live.rs, common/mod.rs, and fixtures/ (10 saved answers)
- /home/user/harnissUI/crates/store/src/sources.rs
- /home/user/harnissUI/src-tauri/src/search_cmd.rs and /home/user/harnissUI/src-tauri/src/search_cmd/tests.rs
- /home/user/harnissUI/src/components/sources/types.ts, api.ts, state.ts, citations.ts, quotes.ts, useCitations.ts, SearchHits.tsx, and tests citations.test.ts, quotes.test.ts, state.test.ts, MarkdownCitations.test.tsx, SearchHits.test.tsx
- /home/user/harnissUI/src/components/SearchSettings.tsx and SearchSettings.test.tsx
- /home/user/harnissUI/src/components/workbench/SourcesTab.tsx (replaces the placeholder) and SourcesTab.test.tsx

Shared files, exactly what I added:
- /home/user/harnissUI/Cargo.toml: member "crates/search"; `search = { path = "crates/search" }`; in [workspace.dependencies] a four-line comment and `dom_query = { version = "0.28", default-features = false }`, `html5ever = "0.39"`, `toml = { version = "1", default-features = false, features = ["std", "parse", "serde"] }`, `encoding_rs = "0.8"`.
- /home/user/harnissUI/src-tauri/Cargo.toml: `search = { workspace = true }`.
- /home/user/harnissUI/crates/policy/src/engine.rs (feature A's file): the variant `Scope::WebSearch` with its doc comment, after `AppData`, and the test `web_search_is_grantable_and_asks_again_after_untrusted_content`. Nothing else.
- /home/user/harnissUI/crates/store/src/schema.rs, inside the [sources] block: one comment line and `CREATE UNIQUE INDEX sources_chat_url ON sources(chat_id, url);`.
- /home/user/harnissUI/crates/store/src/lib.rs: `mod sources;` and one `pub use sources::{…}`.
- /home/user/harnissUI/src-tauri/src/lib.rs: `mod search_cmd;` and six handler entries. The same six in /home/user/harnissUI/src-tauri/commands.list and as `allow-…` in /home/user/harnissUI/src-tauri/capabilities/main.json: get_search_settings, set_search_settings, set_search_key, clear_search_key, test_search, list_sources.
- /home/user/harnissUI/src-tauri/src/toolset.rs: one comment line and `host.add_builtin(Arc::new(crate::search_cmd::SearchServer::new(app.clone())));`.
- /home/user/harnissUI/src/components/Markdown.tsx: `SafeLink` exported; a `Citations` interface, a context, a `Citation` component, one entry in `components`; `Markdown` takes an optional `citations` prop and adds the citation plugin only when it is given.
- /home/user/harnissUI/src/components/MessageItem.tsx: one import, `const citations = useCitations(m.role === "assistant");`, and `citations={citations}` on the answer's `<Markdown>`.
- /home/user/harnissUI/src/components/toolcards.tsx: one import, a comment, two entries (web_search, read_source).
- /home/user/harnissUI/src/components/SettingsDialog.tsx: one import, one `Tabs.Trigger` ("Search", after "Computer use"), one `Tabs.Content`.
- /home/user/harnissUI/e2e/scripted_agent.py: the function `p_search` and `p_search` in `PROBES`.

Dependencies: Cargo.lock gained only the package `search` (checked with git diff). The four crates above were already compiled into the app through Tauri and reqwest and became direct dependencies: dom_query turns a page into a DOM, html5ever lets it be parsed in pieces under a time limit, toml reads Cargo.toml and pyproject.toml, encoding_rs decodes pages that are not UTF-8. `python3 scripts/notices.py --check` passes with the notices unchanged.

Uncommitted at the time of writing (the rest is in your work-in-progress commits): e2e/scripted_agent.py, src/components/sources/SearchHits.tsx, state.ts, useCitations.ts, state.test.ts (new), src/components/workbench/SourcesTab.tsx.

## 2. What it does, and the design choices

As the user sees it:
- The model has two tools: `web_search {query, library?, max_results?}` and `read_source {n, query?}`. Each asks first. The card shows the query, which service it goes to (for SearXNG the host and port) and how many pages are then read. "Allow for this chat" covers both tools.
- A search asks the chosen service, lists official documentation first, reads the top pages (4 by default) from their own sites, and gives the model per hit a number, title, site, marks, address and the best passages. Sources are numbered per chat; an address found again keeps its number.
- In an answer `[n]` is a small chip (title and site on hover); a click opens the Sources tab on that source. `[n]` with no source stays text with a dotted underline and the tooltip "No such source".
- Sources tab, per source: number, title, site, badges ("official documentation", the version note), address, the searches that found it, when the page was read, the SHA-256 of its text, exactly the text the model was given, the quote check ("quote found" / "quote not found in the retrieved text"), and "Open in browser" through the existing link confirmation.
- The card of a search lists its hits (number, title, site); a click opens the source.
- Settings → Search: the service (DuckDuckGo by default, Brave Search, Tavily, SearXNG), key fields (password inputs; only "set" or "not set" is shown), the SearXNG address, pages per search (0 to 8), version matching on or off, "Test search", and a sentence saying where requests go.

Design choices:
- Page requests never use a proxy. Through a proxy the connection goes to the proxy and the proxy resolves the name, so the address check would mean nothing. Consequence: where the web is reachable only through a proxy, pages cannot be read and the model gets the service's summaries with the reason. The request to the search service uses the system's proxy settings; a SearXNG on loopback is asked directly and its redirects are not followed.
- What is stored is what the model was given: the title, the service's summary only when the page was not read, and the excerpt. `read_source` adds the new passages to it. The quote check looks in exactly that text.
- `read_source` fetches the stored address of source n, never an address from the model. It fails if the source changed between approval and run; `web_search` fails if the service was changed in Settings in between.
- The first line of a result (`Sources [1] [2] for: …`) is written by the app. The tool card reads the numbers only from that line, never from text that came from the web.
- Version matching: libraries come from package.json (+ lock), Cargo.toml (+ lock), pyproject.toml, requirements.txt and go.mod in the chat's working folder, read through the path sandbox. When the query or `library` names one, its major.minor is added to the query and its package index (npm, PyPI, crates.io) is asked once per app run for its documentation host, through the guarded fetcher. Notes read "matches tokio 1.38" or "for tokio 0.2", and are given only on that library's own documentation.
- Limits: 10 results, 8 pages, 3 passages of about 700 characters per page, sizes fitted to the chat's result cap, 1,000 sources per chat.
- Citations: a plugin on the parsed Markdown tree, never inside code or links. A single `[n]` always counts; a list `[1, 2]` only when every number is a source. Until the chat's sources are loaded, `[n]` stays plain text and is not marked.
- The sources listener starts on the first read of a chat's sources, not when the module is imported. The first version started it at import and broke the existing ToolCallCard test; I changed my code, not the test.

The exact text of a `web_search` result (pinned by the test `the_result_of_a_search_as_the_model_sees_it`):

```
Sources [1] [2] [3] [4] for: tokio spawn_blocking
Matched to tokio 1.38.2, which this project uses.

[1] spawn_blocking in tokio::task - Rust — docs.rs (official documentation; matches tokio 1.38)
https://docs.rs/tokio/1.38.0/tokio/task/fn.spawn_blocking.html
Runs the provided closure on a thread where blocking is acceptable.

In general, issuing a blocking call in a future is problematic.

…

Closures spawned using spawn_blocking cannot be cancelled abruptly.

[2] Spawning | Tokio - An asynchronous Rust runtime — tokio.rs (official documentation)
https://tokio.rs/tokio/tutorial/spawning
A Tokio task is an asynchronous green thread.

[3] Team wiki: Tokio — intranet.example
http://intranet.example/wiki/tokio
(This page could not be read: intranet.example is a private network address (10.0.0.9), and pages are never read from there. The search service's summary:)
Internal notes about our Tokio setup.

[4] rust - When should you use spawn_blocking? - Stack Overflow — stackoverflow.com
https://stackoverflow.com/questions/61752896
(Not read. The search service's summary; read_source {"n": 4} reads the page.)
Use it for CPU-bound work or for calling synchronous APIs.

Cite a source as [n] right after the sentence it supports. Use only what the excerpts say; if they do not answer the question, say so.
```

When no page could be read, the second line is `None of the pages could be read, so only the search service's summaries are given.` In the running app the loop sends this text unchanged inside its delimiter (captured from the fake endpoint in the e2e run):

```
<tool_output id="99d993f38ef6fa44" tool="web_search" source="web search (SearXNG)">
Sources [1] [2] [3] [4] for: how do herons migrate
None of the pages could be read, so only the search service's summaries are given.

[1] Heron migration routes — 127.0.0.1
http://127.0.0.1:<port>/pages/routes
(This page could not be read: 127.0.0.1 is a loopback (this computer) address (127.0.0.1), and pages are never read from there. The search service's summary:)
Grey herons from the north fly south in autumn and return in early spring.
…
Cite a source as [n] right after the sentence it supports. Use only what the excerpts say; if they do not answer the question, say so.
</tool_output id="99d993f38ef6fa44">
```

## 3. Tests

Rust:
- crates/search: 87 unit tests, 15 fetcher tests and 11 pipeline tests against a small local HTTP server, 4 live tests ignored by default.
- crates/store sources.rs: 7. crates/policy: 1. src-tauri search_cmd: 20.

Fetcher tests by name: every_blocked_class_is_refused_before_connecting (IPv4 and IPv6, including IPv4 inside IPv6), names_that_resolve_to_blocked_addresses_are_refused (also a name with one public and one private answer), localhost_names_are_refused_without_a_lookup, a_redirect_to_a_private_address_is_refused_before_connecting (a public-looking host answers 302 to loopback, private and metadata addresses; the trap server sees no connection), a_page_is_fetched_from_the_address_that_was_checked, redirects_are_followed_by_hand_at_most_five_times, redirects_to_other_schemes_or_nowhere_are_refused, addresses_that_are_not_plain_web_addresses_are_refused, the_request_says_who_asks_and_nothing_about_the_user, plus limits, content types and time limit. The app test the_fetchers_address_classes_agree_with_the_browsers compares my copy of the classification with the browser's over more than 10,000 addresses.

Frontend: 52 tests in 7 files (citations 6, quotes 14, state 7, MarkdownCitations 7, SearchHits 4, SourcesTab 6, SearchSettings 8).

End to end: the scenario `search`.

Seen failing before the code existed:
- All 15 fetcher tests and 77 of the then 86 unit tests of crates/search (the netguard tests came with the copied classification and passed at once).
- The 7 store tests.
- citations.test.ts and quotes.test.ts.
- state.test.ts: 6 of 7 failed before the listener change.
- With `Guard::check` switched off by hand, 4 fetcher tests fail (then restored).

Written with the code and passing on the first run: the 20 app tests, the policy test, and the MarkdownCitations, SourcesTab, SearchHits and SearchSettings tests. The e2e scenario failed once on my own assertion (trailing white space), not on the app.

## 4. Checks run

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo +1.99 clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo +1.99 clippy -p search --lib -- -D warnings` (without the test hooks) | exit 0 |
| `cargo +1.99 test -p search` | 87 + 15 + 11 passed, 4 ignored |
| `cargo +1.99 test -p store` | 99 + 4 passed |
| `cargo +1.99 test -p policy` | 73 passed |
| `cargo +1.99 test -p app` | 232 passed |
| `bash scripts/check-spawn.sh` | passed |
| `python3 scripts/notices.py --check` | exit 0 |
| `npm run lint` | exit 0 |
| `npm run typecheck` | exit 0 |
| `npm test` | 29 files, 187 tests passed |
| `npm run build` | exit 0 |
| `cargo +1.99 build -p app`, then `PROBES=search` under the e2e lock | PASS, 0 console lines |
| `PROBES=markdown,settings-pages,text-tool-call-and-grants,search` | 4 PASS, 0 console lines |

`cargo +1.99 fmt` is not installed for that toolchain; the format check ran with the default one (1.97).

The e2e scenario covers more than the spec asks: the Settings screen shows the choice and "Test search" works; a key is reported only as set and is not in app.db; the approval card names the service; four hits on loopback, `localhost`, 10.0.0.7 and 169.254.169.254 are listed with summaries and the reason; the stand-in server received only the one search request and no page request; chips, the "No such source" mark, the Sources tab with one quote found and one not found; markup in a title is shown as text and nothing ran; `read_source` is refused the same way; the audit log has both calls. Screenshots: /tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/e2e-search/search-sources.png and /tmp/claude-0/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/scratchpad/e2e-search/search-settings.png

Not run: the whole `scripted_agent.py` suite, `m0_close.py`, `m1_chat.py`, `m3_agent.py`, and any build or test on Windows or macOS.

## 5. For ARCHITECTURE.md

Web search (13.6 network use, 9.3 built-in servers, 12 storage). The `search` server has two tools, `web_search` and `read_source`. Both are `Scope::WebSearch`, outbound, network and read-only; they go through `describe`, `policy::evaluate` and the audit log like every tool, one per-chat grant covers both, and they ask again once untrusted content has entered the turn (D9).

Two kinds of request leave the app. The first is the search request to the service the user chose in Settings: the DuckDuckGo HTML endpoint, Brave, Tavily, or the user's own SearXNG address. It carries the query, the User-Agent `<app name>/<version>` and, for Brave and Tavily, the key from the keychain. A SearXNG address may be on loopback because the user set it; it is then asked directly and its redirects are not followed.

The second is every page and package-index request, and it goes through `search::Fetcher`, the only code that reads an address someone else chose. For the first request and for every redirect hop it:
1. allows only http and https, with no user name or password;
2. resolves the name once and refuses if any address it resolves to is loopback, private, link-local, carrier-grade NAT, unique-local, unspecified, multicast or reserved (IPv4, IPv6, and IPv4 carried in IPv6: mapped, compatible, NAT64, 6to4); `localhost` names are refused without a lookup;
3. connects with an HTTP client made for that hop, whose only resolver returns the checked addresses, with no proxy and no redirect following, so there is no second lookup;
4. follows redirects by hand, at most five, each through steps 1 to 3.

Limits: 10 seconds for the whole fetch, 2 MiB of body, `text/html` and `text/plain` only (JSON for the three package indexes), no packed bodies, no cookies, no referrer. The address classification is a copy of `browser::netpolicy::classify_ip`; a test in the app crate keeps the two equal. The cargo feature `test-hooks` opens chosen loopback ports for the crate's own tests; the app does not enable it and does not reference the hook functions.

Everything retrieved is data. Pages are turned into text inside the crate (scripts, styles, navigation, forms and hidden elements dropped; invisible and direction-changing characters removed; parsing stops after 2 seconds or 1,000,000 characters). Titles, summaries and excerpts are sanitised with `policy::sanitize` and capped before they are stored or returned. They reach the model only as a tool result inside the untrusted-data delimiter, and the user only as React text; there is no HTML sink, and citations are made on the Markdown tree, not by building markup.

Storage: table `sources`, per chat, numbered from 1, one row per address. A row holds exactly what the model was given, the SHA-256 of the page's extracted text, and when it was read. `read_source` fetches the stored address of source n and nothing else. Keys are keychain entries `search.brave` and `search.tavily`; commands return only whether one is set. The setting `searchSettings` holds the service, the SearXNG address, pages per search and version matching. Project manifests are read through `PathSandbox`, inside approved folders only.

## 6. Unverified, and things for you to decide

Live search: partly tried, no complete one.
- Worked from this environment (earlier in the session, `cargo test -p search --test live -- --ignored`): the npm, PyPI and crates.io index lookups, and reading one real page through the guarded fetcher.
- Did not work: DuckDuckGo. `html.duckduckgo.com` answered 202 with its bot check page, or reset the connection. The app reports that as "DuckDuckGo refused the search: it turns automated searches away for a while. Try again later, or …".
- So the DuckDuckGo results parser has only ever seen a fixture I wrote by hand from the known page structure, never a real results page. If the real markup differs, the default service returns no results. This is the largest open risk, because DuckDuckGo is the default. I do not know whether the refusal is about this environment's address or about the plain User-Agent; I did not try a browser-like one, since the app should not pretend to be a browser.
- Brave and Tavily: parsers tested only against fixtures written from their documented answers; no keys to try them.
- SearXNG: tried only against a local stand-in (unit, integration and e2e), not a real instance.

Other gaps:
- In the running app only the path where pages are refused was exercised. Reading real pages is covered at crate level (local server, and the one live page), and the tab's display of excerpt, hash and read time by component tests.
- Not built or run on Windows or macOS. The crate has no platform-specific code.
- On Linux the app's secret store is in memory (existing behaviour), so the e2e key check proves "not in the database", not the OS keychain.

For you to decide:
- D9: after any tool result in a turn, a further `web_search` or `read_source` asks again even with the grant. Your interim note says you intend to change this.
- A single `[n]` in prose outside code always counts as a citation, so in a chat without sources, text like `a[1]` written without code marks gets the dotted underline. I kept it because it also marks invented citations in chats that never searched.
- robots.txt is not consulted; the app reads at most 8 pages per approved search.
- The e2e scenario `settings-pages` does not include the new "Search" tab in its list; my scenario opens it instead.
- The live tests send the User-Agent `harnessUI/<version>`, as the app does.
- Feature E's catalog already has both tools (group label "Web search"), so my earlier note about the raw group name no longer applies.

TODOs left in code: none.
