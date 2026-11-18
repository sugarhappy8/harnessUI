# Hugging Face answer fixtures

Answers in the shape of `GET /api/models/<repo>?blobs=true&expand[]=…` (the fields
`HfClient::repo` asks for) and of the search API.

## Made up

Every name, number and hash is invented; they exist so the parsing of what an uploader
lists (0.0.6 item 4) and Studio's recognition of a repository can be tested without the
network.

- `repo-full.json`, `repo-hostile.json`: `repo-hostile.json` holds the kinds of text an
  uploader could use to mislead: a right-to-left override, zero-width characters, markup,
  over-long text and links that are not https.
- `repo-klein-4b-name-9b-parameters.json`: a repository named like FLUX.2 klein 4B whose
  listing states 9 billion parameters.
- `repo-unknown-diffusion.json`: a picture model of no family harnessUI knows.
- `repo-uncensored.json`: a Z-Image Turbo re-upload tagged `uncensored`.
- `repo-no-hashes.json`: a Z-Image Turbo repository whose files have no LFS SHA-256.

## Recorded on 2026-11-18 (0.1.0 Studio)

Real answers from huggingface.co, recorded the day the catalog's picture and video parts
were pinned (`catalog/models.json` takes every revision, size and SHA-256 from these, and
`gen_search::tests::every_part_repo_is_its_listings_canonical_id` checks that it does).
Only the whitespace was changed (indented), except where a table below says an item was
removed. Each repository's `id` is the canonical one its listing gives: an old name
(Comfy-Org's `flux2-klein-4B`, `z_image_turbo`, `Wan_2.1_ComfyUI_repackaged`) answers with
a redirect.

| File | Repository |
|---|---|
| `repo-flux2-klein-4b-gguf.json` | leejet/FLUX.2-klein-4B-GGUF |
| `repo-flux2-klein-9b-gguf.json` | leejet/FLUX.2-klein-9B-GGUF |
| `repo-z-image-turbo-gguf.json` | leejet/Z-Image-Turbo-GGUF |
| `repo-fastwan22-ti2v-5b-gguf.json` | Green-Sky/FastWan2.2-TI2V-5B-FullAttn-GGUF (no pipeline tag) |
| `repo-wan22-ti2v-5b-full-gguf.json` | QuantStack/Wan2.2-TI2V-5B-GGUF (the full Wan 2.2 5B, and the Wan 2.2 decoder) |
| `repo-qwen3-4b-gguf.json` | unsloth/Qwen3-4B-GGUF |
| `repo-qwen3-4b-instruct-2507-gguf.json` | unsloth/Qwen3-4B-Instruct-2507-GGUF |
| `repo-comfy-flux-2-klein-4b.json` | Comfy-Org/Flux-2-Klein-4B |
| `repo-comfy-z-image-turbo.json` | Comfy-Org/Z-Image-Turbo |
| `repo-comfy-wan-2.1.json` | Comfy-Org/Wan-2.1 (the Wan 2.1 decoder) |
| `repo-umt5-xxl-encoder-gguf.json` | city96/umt5-xxl-encoder-gguf |

The search answers are the three queries `HfClient::search_generators` sends
(`filter=gguf`, `sort=downloads`, `limit=30` and the `expand[]` of `HfClient::search`):

| File | Query |
|---|---|
| `gen-search-text-to-image.json` | `pipeline_tag=text-to-image`, `search=klein` |
| `gen-search-image-to-video.json` | `pipeline_tag=image-to-video`, `search=wan2.2 5b` (holds Turbo repositories) |
| `gen-search-untagged.json` | no pipeline tag, `search=5b` (holds Green-Sky's FastWan, which has no pipeline tag, and chat models; one unrelated chat repository was removed from the recorded answer, so it holds 29 items) |
