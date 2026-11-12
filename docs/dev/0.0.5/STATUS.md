# 0.0.5 status and how to resume

Kept up to date by the lead so the work can be picked up after an interruption (for example
when the account's usage limit pauses the session).

## Where things are (2026-11-12, 13:15 UTC): STOPPED BY THE OWNER
- Design: done. `spec.md` (revision 2 plus "Amendments after WP0"), `critique.md`, `map.md`,
  `map-details.md`.
- WP0 groundwork: done and independently checked (`report-WP0-*.md`; the contract is the
  section "CONTRACT (updated)" of `report-WP0-fix.md`). Committed in aabd67e.
- Feature packages: running since 09:48 UTC as four workflows of the same script
  (build → security review + correctness review → fix), all editing this one tree:
  | Workflow run | Packages |
  |---|---|
  | wf_d8cbef87-693 | WP1a, WP1b |
  | wf_301d26dc-b08 | WP3a, WP3b, WP4a, WP4c |
  | wf_40078dc2-223 | WP5a, WP6 |
  | wf_40f806d9-784 | WP2, WP4b, WP5b |
  Script: `/root/.claude/projects/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/workflows/scripts/v005-packages-wf_d8cbef87-693.js`
  (the package briefs are the `args` of each run; they are also in this session's record).
- 10:40 UTC: the usage limit stopped every builder about 50 minutes in (only WP4b's build
  finished). 12:28 UTC: all four runs resumed with `resume: true` for every unfinished package
  (WP4b's build replays from cache; its reviews run). Resume arguments:
  `scratchpad/v005/resume-args.json`.
- 13:15 UTC: the owner said "stop". All four runs were stopped and the hourly check was
  deleted. The builders' partial work is committed (no package had finished its build except
  WP4b; WP1a and WP3b were in their final checks). To continue later, follow "How to resume"
  below with `resume: true` for every package (WP4b: its reviews and fix still to run).
- Work in progress is committed with `[skip ci]` about every hour; the builders never commit.
- Shared dev server for end-to-end runs: `npm run dev` on port 1420 (restart it if it is gone).

## How to resume after an interruption
1. `git status` / `git log -3`: commit anything uncommitted with `[skip ci]`.
2. Read each run's `journal.jsonl` under
   `/root/.claude/projects/-home-user-harnissUI/23ef2267-cb6f-5325-b082-a4b40b3ccb19/subagents/workflows/<run>/`
   to see which agents finished with a result and which died (an agent that hit the usage
   limit returns nothing).
3. In the same session: relaunch with `Workflow({scriptPath, resumeFromRunId: <run>, args})`
   using the same `args`, adding `"resume": true` to every package whose build, review or fix
   did not finish. Finished agents replay from cache; packages marked `resume` get a note
   telling the developer that part of the package may already be in the tree. In a new
   session (no cache): run the script again with only the unfinished packages, each marked
   `resume: true`.
4. When all packages are done: the lead's integration (spec Part 4): cross-package
   end-to-end runs, the whole workspace and frontend checks, every e2e suite (scripted
   scenarios plus m0, m1, m3, m9, m10 with the real small model), a final whole-release
   review, docs (ARCHITECTURE, CHANGELOG, README, SECURITY), version 0.0.5, CI on all three
   systems, the pull request, and the owner checklist (spec Part 4).

## Decisions taken for the owner (they may change any)
See spec.md Part 2, D1 to D11. The owner chose "both directions" for item 3.
