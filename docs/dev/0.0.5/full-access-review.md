# 0.0.5 Full access: independent review and what changed

The owner's request: "stop asking me to allow everything ... name it full access", plus
"the ability for the models to create their own folders and navigate the pc on their own
and stuff even delete and organize but this only works with full access on", with the rule
"run absolutely everything; only the app's own database and saved keys stay protected".

After the first version was built, an independent reviewer read the change and probed it
with throwaway tests. Its findings and what was done about each:

| # | Finding | What changed |
|---|---|---|
| 1 | A command could write the app's database (`sqlite3 …/io.github.*/app.db`, the glob hid the identifier) and make Full access the saved default, applied at the next start with no confirmation. | A saved Full access default is never applied by itself: every chat starts in Ask first and the main window asks "Start every chat in Full access again?" (`approvals::answer_full_access_default`). |
| 2 | With `/` reachable, `search_files`, `write_file` and `edit_file` could read `/dev/zero` (endless) or a pipe (blocks for ever). | The file tools refuse devices, pipes and sockets; search reads regular files only, never past its limit, and skips `/proc`, `/sys`, `/dev`. |
| 3 | The browser is shared by every chat; Full access released any held request, also one from another chat's page or outside a call. | Full access releases only holds of the chat's own call (`during_tool` set). |
| 4 | The terminal guard read text only: globs, `~`, variables and parent folders (`rm -rf ~`) got past it. | `terminal::reaches_app_folder`: words read as paths (expansions, `..`, globs matched as a shell would or more); a path inside an app folder is refused, and so is a delete, move or change of rights on a folder that holds one. What a program does once it runs is still out of sight; the docs say so. |
| 5 | "Saved keys stay out of reach" was claimed too broadly; the key stores themselves were reachable by the file tools. | The system's key-store folders are forbidden to the tools; the wording in the app, README, CHANGELOG and SECURITY.md now says what holds: out of reach of the file tools, while a program the model runs has the user's rights (on Windows it can read Credential Manager entries without a prompt). |
| 6 | `move_path` on a link moved the link's target; a hint pointed at `delete_file`, which would delete the target. | `move_path` refuses links; the hints changed. |
| 7 | Going back to 0.0.4, switching "Allow all in every chat" off, and upgrading again brought Full access back. | If 0.0.4 switched it off after 0.0.5 saved a mode, the mode is Ask first. |
| 8 | The drive list was worked out on every turn, also outside Full access (a stale network drive on Windows could stall every turn). | Worked out the first time a Full access call needs it in the turn. |
| 9 | On macOS only `Contents/MacOS` of the app bundle was forbidden. | The whole `.app` bundle is the install folder. |
| 10 | Changing the default overrides chats switched by hand. | Kept (documented, and the confirmation says so). |
| 11 | "folder" and "organize" brought `delete_file` into every chat's tool list. | Those words only rank the folder tools now. |
| 12 | Chats with a tool list saved before 0.0.5 did not get the folder tools in Full access. | The folder tools come with `write_file` in Full access instead of being switches of their own. |
| 13 | Small things (a self-contradicting refusal text, a misplaced doc comment, `/usr/bin` as an install folder on Linux .deb builds). | Left as they are for now; Linux is a development platform. |

## Found by the owner during the work

On their Windows PC, commands looked as if they did not run: a program that wrote to its
error stream (Node's ExperimentalWarning, npm's notices) was reported as failed with exit
code 1, and the output was buried in PowerShell's CLIXML (`<Objs Version="1.1.0.1" …>`).
Windows PowerShell turns each line of a program's error stream into an error record, which
clears its success flag, and with `-EncodedCommand` it writes error and progress records to
a redirected stderr as CLIXML. A failure is now the program's own exit code or an error of
PowerShell itself, progress is off, and leftover CLIXML is decoded to plain text
(`terminal::decode_clixml`). Tests in `terminal::windows_tests` run real Windows PowerShell
on CI.

The owner also suspected the one-folder limit. In Ask first and Allow all the tools are held
to the approved folders by design (a command can still `cd` elsewhere); Full access lifts
that limit.

## Checks run

- `cargo test --workspace` (all 40 test groups), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, the spawn and notices checks.
- `npm run lint`, `npm run typecheck`, `npx vitest run` (263 tests), `npm run build`.
- End-to-end: all 29 scripted scenarios (including the new `full-access` and the updated `allow-all` and `tool-selection`), and the suites for app close, chat, the agent with a real model, skills, the browser and the model check.
