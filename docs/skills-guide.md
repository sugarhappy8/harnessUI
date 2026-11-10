# Writing skills

A skill is a folder with a `SKILL.md` file: instructions the model reads when a task needs
them, plus any scripts and reference files they mention. harnissUI reads the same format
other agent apps use, so most skills work as they are.

## Folder layout

```text
pdf-tools/
  SKILL.md        required
  scripts/        optional: programs the instructions tell the model to run
  references/     optional: longer documents the model reads when it needs them
  assets/         optional: templates and other files
```

Only `SKILL.md` is required. Files under `scripts/` get a ready-to-run command when the
skill is loaded (see [Scripts](#scripts)); the other folders are just files the model can
read.

## SKILL.md

`SKILL.md` starts with YAML frontmatter between two `---` lines, followed by Markdown
instructions:

```markdown
---
name: pdf-tools
description: Extract text and tables from PDF files. Use when the user asks about the contents of a PDF.
license: MIT
compatibility: Needs Python 3.
metadata:
  author: someone
allowed-tools: Bash Read
---
# PDF tools

To get the text of a PDF, run scripts/extract.py with the file's path.
For tables, read references/tables.md first.
```

The format has six fields:

| Field | Required | Rules |
|---|---|---|
| `name` | yes | 1–64 lowercase letters, digits and single hyphens, not starting or ending with a hyphen. Must match the folder name. |
| `description` | yes | 1–1024 characters. Say what the skill does and when to use it: this is all the model sees before it loads the skill. |
| `license` | no | Text. |
| `compatibility` | no | Text, at most 500 characters. |
| `metadata` | no | Keys mapped to text values. |
| `allowed-tools` | no | Shown in the skill's details only. It never turns a tool on, grants anything or skips an approval card. |

**Strict and lenient checks.** The editor in *Settings → Skills* (*New skill*, or the
pencil on a skill you added) applies the rules above strictly: *Validate* lists the
problems, any other field is an error, and *Save* writes nothing until the list is empty.
Skills you import are loaded leniently: a name that does not match its folder, an unknown
field or a metadata value that is not text only shows a warning next to the skill. A skill
does not load at all if `SKILL.md` has no frontmatter, the YAML cannot be read or there is
no description; *Settings → Skills* lists it with the reason.

## What the model sees

- **The index.** The name and description of every enabled skill are in each prompt.
  Nothing else from the skill is sent until the model loads it.
- **`load_skill`** returns `SKILL.md`, the skill's folder and its list of files, with a
  command for each script. It never shows an approval card and is recorded in the audit log
  as allowed without a prompt. The skill's text then stays in that chat's instructions; the
  chat's *Tools* panel lists loaded skills with an *Unload* button.
- **Size limit.** All skills loaded in one chat together may use about 30% of the model's
  context (counted at about three characters per token). A skill that would go over is not
  loaded, and the model is told to unload another one or use a model with a larger context.
  Keep `SKILL.md` short and move long material into `references/`.
- **`read_skill_file`** reads one file inside the skill's folder, in pages of lines for
  long files. Paths outside the folder are refused. Binary files report only their size.

## Scripts

Scripts run only through the terminal tool (`run_command`), in the skill's folder, with the
terminal's usual approval cards. The terminal tool is off in a new chat: when it is off, the
model is told the skill's scripts cannot run and to ask you to turn it on.

The command the model is given depends on the file type:

| File | Windows | macOS |
|---|---|---|
| `.py` | `python` | `python3` |
| `.sh` | `bash` (needs Git Bash or WSL) | `bash` |
| `.ps1` | runs in PowerShell | `pwsh -File` (needs PowerShell 7) |
| `.js`, `.mjs`, `.cjs` | `node` | `node` |
| `.ts` | `npx tsx` | `npx tsx` |
| `.rb` | `ruby` | `ruby` |

The terminal is Windows PowerShell 5.1 on Windows and zsh on macOS, and the app installs no
interpreters. For a skill meant for both systems, write scripts in Python or Node.js, or
ship a `.ps1` and a `.sh` and say in `SKILL.md` which one to use where.

## Adding skills to the app

- **Import.** *Settings → Skills → Import folder…* or *Import zip…*. In a zip, `SKILL.md`
  must be at the top level or inside one top-level folder. Absolute paths, `..`, symbolic
  links, more than 512 files or more than 16 MiB unpacked are refused, as is a skill whose
  name is already installed.
- **Imported skills start turned off.** A skill's text becomes instructions to the model,
  so turn one on only if you trust where it came from.
- **Create.** *New skill* opens the editor with a starter `SKILL.md`. To add scripts or
  reference files, use *Open skills folder* and add them with your own file manager or
  editor. The model's file and git tools cannot change anything in a skills folder.
- **Shared folder.** *Settings → Skills* can also read `~/.agents/skills`, the folder other
  agent apps use. Skills there can be turned on and off but not edited or deleted in the
  app.

## Trying a skill

Turn the skill on, start a new chat and ask for the task its description names, or say
"load the skill pdf-tools". A `load_skill` card appears without asking. If the model does
not pick the skill up by itself, make the description say more plainly when to use it.
