# 0.0.6: what only your computers can show

0.0.6 was built and tested on a Linux computer with no graphics card. Everything below needs
your Windows PC (AMD Radeon RX 9070 XT) or your 16 GB Mac. Each line says what to do, then
(→) what should happen. Write OK, or what you saw instead (a screenshot helps). If a model
fails to load, send the last lines of the log too: *Settings → Diagnostics → Open logs folder*.

**Before you start** (the lead gives you these):
- the 0.0.6 installers from CI (Windows and Mac);
- the folder "rea test files" (from the *Reverse engineering* workflow on GitHub); put it
  inside a folder you approve for a chat;
- the table of quotes behind each catalog model's "What is it good at?" (section "Either");
- some models: one that fits easily (about 8 billion parameters, Q4), one bigger than 16 GB,
  one that can read pictures, one that thinks before answering;
- Claude Code and Codex installed (with npm) and signed in, for the "Other apps" and
  "Coding apps" parts.

---

## Windows PC (Radeon RX 9070 XT)

**Model settings (the gear, *Settings for this model*)**
1. *Use the graphics card*: try *All of it*, *As much as fits*, *Don't use it*, each with
   *Apply and restart* → each loads; *As much as fits* says part runs on the processor;
   *Don't use it* is slower. The marks beside the choices change before you press Save.
2. *More options → Which graphics card* → the card is named "AMD Radeon RX 9070 XT · 16 GB",
   never "Vulkan0"; a second (built-in) graphics chip, if any, is listed by its own name and
   the RX 9070 XT is marked recommended.
3. A model of about 8 billion parameters (Q4) on *Automatic* → the panel says "Automatic:
   32,768 tokens…", it loads with no step-down, and the header's "Remembers about … words"
   matches the panel.
4. *Save memory* on, Apply and restart → it loads; if the card refuses it, the header says
   "Save memory doesn't work with this graphics card, so it was turned off for this load."
5. *Keep the model in memory* on → it loads, and nothing else on the PC misbehaves.
6. A model bigger than the card with *All of it* → the loading line says it is trying again
   with part of the model on the processor, then it is ready with a note why replies are
   slower; it never ends with "Didn't load". Then *Reload* → it tries the graphics card again.
7. *Don't use it* with a memory size far too large → one plain out-of-memory message, no
   graphics-card note, no second attempt.
8. While a reply is being written, press *Reload* → "A reply is being written. Stop it and
   restart the model?" with *Cancel*, *Restart when it ends*, *Stop and restart*; each does
   what it says.
9. *Settings → Diagnostics → Run backend check* with a model loaded, and pick another model
   meanwhile → "A model check is running…"; *Restart when it ends* loads the model you picked
   after the check, and the old one does not come back over it.
10. With a thinking model, *How long it may think*: *Short* → it starts answering sooner
    than with *Long*.
11. At your usual window size, in a chat with messages → the header shows part of the model
    name and "Remembers about N words" (or its short form).
12. Optional: choose the graphics card, then disable it in Device Manager and load → "…the
    chosen graphics card was not found. Choose another in Model settings." (enable it again).

**Long chats**
13. Attach a long table of numbers or code → the reply comes, with a note that only the first
    part was sent.
14. Chat until the line "Above this line, the model sees a summary…" appears; close and reopen
    harnessUI; *Show summary* → shows the summary.
15. With the memory size at 32,768, chat until it summarises, then set 8,192 and *Apply and
    restart* → the chat goes on.
16. After a "too long" error (also with pinned instructions that do not fit) → the
    *Model settings* button opens that model's settings.
17. With a model that reads pictures, attach a WebP picture → the model describes it; Task
    Manager shows no ffmpeg or ffprobe.

**How busy the computer is, and Fine control**
18. No model loaded → under the model name: "Graphics card N% · x of 16 GB · Processor N% ·
    Memory x of y GB", with numbers (not "–"); no administrator prompt; its menu names the
    RX 9070 XT.
19. While a reply is written, compare with Task Manager → graphics card within about 10 points
    of "GPU", graphics memory within about 0.3 GB of "Dedicated GPU memory", processor within
    about 10 points.
20. A chat open and nothing else running → harnessUI under 2% of the processor in Task Manager.
21. Make the window narrow → the usage line shortens to icons with numbers; nothing overlaps.
22. Open *Settings for this model* → "Where the model goes" appears at once; open *Fine
    control* → within 5 seconds it says "Worked out for these settings."
23. Load that model with the same settings → note the picture's "Working space" on the
    graphics card and the figure in the usage menu (both numbers).
24. *Layers on the graphics card* = 20 of N, Load → the menu says "20 of N layers on the
    graphics card"; less dedicated memory in Task Manager; slower replies.
25. *All of it*, Load, then set *Layers on the graphics card* to its maximum and Save → the
    header does not say "Settings changed".
26. Only if the PC has 32 GB of memory or more: a model built from expert parts and bigger
    than 16 GB (in Browse, a name with "A3B", about 30 billion parameters, its Q4_K_M file of
    about 18 GB) → with *All of it* the picture warns and the load steps down; with *Expert
    parts on the processor* = all of them it loads with every layer on the graphics card and
    replies at a usable speed.
27. *Conversation memory on the graphics card* = No → dedicated memory drops by about the
    picture's conversation-memory figure.
28. *Processor threads* = 4 with *Don't use it* → lower processor use than with Automatic.
29. *Fast attention* On, then Off → both load (if On fails, the note says it was turned off
    for this load).
30. *Leave free on the graphics card* = 4 GB with *As much as fits* → fewer layers than with
    Automatic.
31. Too many layers for the card → a warning; *Load anyway* → it loads, or says "The N layers
    you chose didn't fit on the graphics card, so As much as fits was used for this load."
32. After a load with *As much as fits* → the menu's "About N of M layers" is within a layer
    or two of the picture before loading.
33. With the AMD graphics card (ROCm) download → the picture says "Rough estimate from the
    model file. The AMD graphics card (ROCm) download can't work out exact figures."
34. *Models → Compare* a model set to *As much as fits*, at a size that spills over → "Needs
    about X GB in all: Y GB on the graphics card, which has Z GB free, and W GB in the
    computer's memory", matching the two bars in its settings; at a size too large, the line
    names the computer's memory and its free figure matches the second bar.
35. If llama-server ever crashes → note whether a Windows Error Reporting window appears.

**Other apps use your model (Settings → Other apps)**
36. Switch it on, *Show key*, *Copy* → the setup card opens on the Windows tab; Copy works.
37. Choose a port another program uses → "Port N is used by another program. Choose another
    port."
38. Claude Code, *For this window only* (PowerShell), then *For every window* → Claude Code
    answers with your local model; the header says the model is used by another app; *Audit
    log* lists each request without its text. Remove the lines → Claude Code uses your account
    again. Also with `ANTHROPIC_API_KEY` set → it still reaches harnessUI.
39. Codex, *Only when you ask for it* (`codex --profile harnessui`), then *Every time* with the
    lines pasted at the top of a config file that already has settings and sections → Codex
    answers with your local model.
40. The Claude app's setting (marked "Not tried yet") with the `http://` address → note
    whether it works.
41. *Make a new key* → apps set up with the old key stop working until you copy the setup
    again; *Audit log* shows the refused requests readably.
42. A model that remembers less than 32,768 tokens → the page says so and offers the size.

**Claude Code and Codex from a chat (Tools → Coding apps)**
43. Turn on *Coding apps (experimental)* → the panel shows where it found each program (the
    real `.exe` where there is one).
44. Ask the model to hand a small task to Claude Code → a card with *Allow once* / *Deny*
    saying what it may read, that it runs online with your account, and that what it reads
    goes to Anthropic. *Deny* runs nothing; *Allow once* runs it, shows "Claude Code is
    working… N min · Stop", and the answer comes back. *Stop* ends it.
45. The same with Codex → the card also says Codex's sandbox has not been tried on Windows.
    In a folder that is not a Git repository → "Codex works only in a folder that is a Git
    repository or that you trust in Codex…".
46. In Full access → it runs without a card.
47. Claude Code set up for harnessUI (step 38) with sharing off → refused in plain words
    before any card; with sharing on → the card says the task goes to your local model.
48. In Allow all, ask the model to run `claude -p hi` in the terminal → it asks (*Allow once*
    / *Deny*).

**Reverse engineering (Settings → Reverse engineering)**
49. Wi-Fi off, *Download* rea → "Can't reach …". Wi-Fi on again.
50. *Download* rea (about 80 MB) → progress, then "Installed" with versions and sizes; Kuna is
    greyed with "rea can't use Kuna on Windows yet."; the tab says "On Windows, Ghidra is the
    decompiler."
51. With rea only, in a chat whose folder holds the test files, ask "Which functions from other
    libraries does hello-win-x64.exe use?" → a card, *Allow once*, an answer.
52. *Download* Ghidra and Java (about 775 MB); *Cancel* partway, then *Resume* → it continues
    where it stopped.
53. Copy `C:\Windows\System32\notepad.exe` into the approved folder; ask "Open notepad.exe and
    show me the readable code of its first function." → a card naming the file and Ghidra;
    *Allow once*; code appears (note how long the first open takes). Another function → no
    card.
54. Drop `notepad.exe` from that folder on the message box → its path appears in quotes; drop
    one from a folder that is not approved → a note with *Add a folder…*; drop a text file
    (also one with Windows line endings) → nothing changes; in a new chat (group off) → a note
    to turn the group on, and *Open Tools panel* opens it; drop a file from harnessUI's own
    data folder → "rea can't reach …".
55. Ask about a file outside the chat's folders → refused, no card.
56. Wait more than 15 minutes, ask about another function → no card; it says rea is opening
    the file again; code comes back.
57. Press *Stop* while rea opens a file → within about 10 seconds Task Manager shows no
    `java.exe` or `decompile.exe` from harnessUI.
58. Wi-Fi off, restart harnessUI, repeat step 53 → it works.
59. If Windows or your antivirus blocks part of rea or Ghidra → note the message, then
    *Repair*. *Update* or *Repair* while rea is idle → works without restarting harnessUI.
60. Open a file in one chat, then another file in a second chat; ask the first chat about its
    file → it says rea closed it because another chat opened one.
61. After any rea failure, *Show details* → no path of harnessUI's own folders in it.
62. Full access: a file on the Desktop → opens without a card (note whether a OneDrive Desktop
    works); a file on a USB stick → note the message; a file in harnessUI's own data folder,
    the Desktop's parent folder as a folder, or `%LOCALAPPDATA%` as a folder → refused.
63. With the lead: while Ghidra works, Resource Monitor → Network → Listening Ports shows
    `java.exe` only on 127.0.0.1 (or nothing); with harnessUI's connections and name lookups
    recorded from its start, a chat and step 53 → nothing beyond the list in ARCHITECTURE.md
    13.6. Also: replace a subfolder holding an open file with a link or junction to
    `\\host\share`, then ask about another file → rea's file is closed and no network
    connection is made.
64. Before setup the *Tools* panel offers "Set up reverse engineering"; with only rea
    installed it says a decompiler is needed. *Remove* Ghidra, then rea → "Not installed" and
    the space is freed.

**The chat's model list**
65. Download a model in *Models → Browse* and, without loading it, open the header's model
    menu → it is under "On this computer".
66. The file from your screenshot → listed once if it is one file, or twice with the end of
    each folder ("Downloads\models" and "…\models") if there are two copies; picking each
    loads that file.
67. *Open a GGUF file…* from a mapped network drive, then pick it from the menu in another
    chat → it does not load again.

---

## Mac (16 GB, Apple silicon)

**Model settings, long chats, usage**
1. The setting reads *Use Apple graphics*; its help lines and notes say "Apple graphics",
   never "graphics card".
2. A model of about 8 billion parameters (Q4) on *Automatic* → about 7,000 tokens; with *Save
   memory* about 14,000; it loads with no step-down.
3. *Save memory* on → it loads (or the header says it was turned off for this load).
4. *Keep the model in memory* on → it loads (the help says it is not recommended with 16 GB).
5. Steps 10, 13 to 17 of the PC list (thinking time, long chats, a WebP picture).
6. Under the model name → "Apple graphics N% · Processor N% · Memory x of 16 GB"; no password
   prompt; Apple graphics roughly follows Activity Monitor's GPU History during a reply;
   memory roughly matches "Memory Used"; harnessUI under 2% of the processor with a chat open.
7. *Settings for this model* → one memory bar with "Apple graphics can use up to … GB"; *Fine
   control* figures within 5 seconds.
8. *Layers on Apple graphics* = half → it loads, the menu says so, replies are slower.

**Other apps and coding apps**
9. *Settings → Other apps* → the card opens on the Mac tab; *Show key* (the keychain may ask
   first); *Copy*; a port in use → "Port N is used by another program. Choose another port."
10. Claude Code (*For this window only* in Terminal, and *For every window*), Codex (both
    ways) and the Claude app → as PC steps 38 to 42.
11. *Coding apps* with Claude Code and Codex installed the usual way (npm or Homebrew) → both
    are found, even though harnessUI was opened from the Dock; PC steps 44 to 48 behave the
    same (no Windows sandbox line).
12. In Allow all, ask the model to run `/usr/local/bin/(x|claude) -p hi` in the terminal (zsh)
    → it asks.

**Reverse engineering**
13. If Apple's command line tools are "Not installed": the rea row says "Can't be used until
    Apple's command line tools are installed"; *Download* rea and Kuna (about 120 MB); ask the
    model to open `hello-macos-arm64` → Apple's install window must not appear; the reply says
    to install Apple's tools.
14. *Install* beside Apple's command line tools, follow Apple's window → the tab says
    "Installed" without restarting harnessUI.
15. The Ghidra row is greyed with "Not available on Mac in this version."
16. Ask the model to open `hello-macos-arm64` and show `main` → a card, *Allow once*, code;
    follow-up questions show no card. Again with Wi-Fi off after a restart → works.
17. Ask it to open `hello-macos-universal` (built for both chips) → the reply says Kuna can't
    open this kind of file and what to do.
18. Wait more than 15 minutes, ask about another function → no card, code comes back.
19. Press *Stop* during an open → within about 10 seconds Activity Monitor shows no `kuna` or
    `java` from harnessUI (a `kuna` left over is a known limit: note it).
20. Drop `hello-macos-arm64` and an `.app` from an approved folder on the message box → their
    paths appear; before rea is installed nothing happens; a file whose name holds a
    right-to-left mark (copy one from the test files) → a note to rename it, not the path.
21. After a rea failure, *Show details* → no path of harnessUI's own folders (no
    "Application%20Support/…/reverse").
22. With the lead: record harnessUI's connections and name lookups during step 16 → nothing
    beyond the list in ARCHITECTURE.md 13.6. Replace a subfolder holding an open file with a
    link to `/net/<host>/share`, ask about another file → rea's file is closed, nothing is
    mounted.
23. *Remove* everything → the space is freed.

**The chat's model list**
24. Download a model without loading it → it is in the header's model menu; with many models
    installed the menu scrolls; remove one in *Installed* → it leaves the menu.

---

## Either computer

1. **Publisher quotes (before release).** Read the lead's table: for each catalog model, the
   sentence shown under "What is it good at?", the phrase behind each "good at" word and the
   link to the publisher's own model card. Approve or swap each one. In the app, add a model
   from *Models → Compare → Suggested models*: its column shows the same text, marked "From
   the publisher's model card".
2. *Models → Compare*: a suggested model next to an installed one → for the same file and
   size, "Will it fit on this computer?" matches the Browse page.
3. With the internet, add a catalog model that is not downloaded → its column fills within a
   few seconds; without the internet → "These facts need the internet. They are saved once
   the model is downloaded."
4. Download a model from Browse, and one from *Suggested models* or first run, without
   comparing them first; turn the internet off and compare them → the Hugging Face rows are
   still filled.
5. Add a Browse result → a file is picked for you; *Change* picks another; *Show model card*
   asks before opening the browser; change *Compare at* several times quickly → the columns
   settle on the last size.
6. The folder chip in the chat header → shows the chat's folder ("No folder · Choose" when
   there is none); in Full access it says "Whole computer". The *Tools* panel's note under
   *Folders for this chat* says the same in Full access.
7. With the home folder approved and `~/.local/bin` a link to a folder outside it → turning on
   *Coding apps* is refused.
8. The system keychain (Windows: Credential Manager; Mac: Keychain Access) holds harnessUI's
   sharing key once sharing was switched on.
9. On a computer with 16 GB of memory or less, the Ghidra row says "With a large model loaded,
   your computer may slow down while Ghidra works."

---

## For the lead (not the owner)

- CI on pull request #5 green on Linux, Windows and macOS (Windows and macOS have never run
  0.0.6's tests). In the Windows and macOS logs, read the sample line of
  `reports_plausible_values_here` (usage readers) and check `cargo test -p reverse --lib
  env::tests` passed on Windows.
- Run the *Reverse engineering* workflow (`reverse.yml`) by hand once it is on `main`; it
  makes the `rea-test-files` artifact for the owner. Until it has run, its Windows and macOS
  jobs are unverified.
- With a vision model and its image reader: `TEST_VISION_GGUF=… PROBES=share-webp` (WebP
  through the sharing proxy).
- Hand the owner WP4c's table of quotes (it lives in that package's report, not in the
  repository, because docs name no models).
