# Real backend answers

What the app's backend really returned for a few public Hugging Face repositories, captured
for the blank-window bug of 0.2.0 (Get models: opening a model with more than one model type
left the whole window blank). Each folder is one repository, named `owner__name`:

- `repo.json`: the answer to `studio_local_repo` (files, verdicts, plans, licences).
- `plan.json`: `{ file, plan }`, the answer to `studio_setup_plan` for the first file choice.
- `card.json`: the repository's search hit, where it is not in `search.json`.

`search.json` is the answer to `studio_local_search` for "qwen image uncensored".

They hold public repository data only: no account, token, path or computer of anyone's. The
tests (`GetModelsBlank.test.tsx`) and the preview harness's `?real=1` flag
(`e2e/preview/README.md`) read them. Keep each file small (under 200 KB).
