# 0.0.7: what only the owner's computers can show

Each package adds what it could not check here (no Windows PC or Mac in the build machine).

## Start here: the five checks that matter most (about 15 minutes)
1. **Computer use:** load Qwen 3.8 27B with its image file (or any model that can see), switch
   on *Computer* in the chat's *Tools* panel, keep harnessUI in front and ask "Open Notepad and
   type hello". harnessUI minimises itself, the text lands in Notepad, and harnessUI comes back
   for each card and at the end (details: *Computer use (P3)* below, item 1).
2. **The model knows its tools:** in a new chat ask "what tools do you have?" — it names them,
   also the ones it isn't shown this turn (details: *Tools, call guide, repeat limit (P4)*).
3. **Memory estimates:** *Models → Browse*, `gemma-4-12B-coder…heretic` at 65,536 tokens: no
   longer "Too large" (details: *Memory estimates (P1)*, item 1).
4. **Broken image file:** load Ministral 3 14B Reasoning Uncensored with its image file: it
   loads, and the header says it was loaded without image input (details: *Image files (P2)*,
   item 1).
5. **Cloud keys (if you have one):** *Settings → Endpoints → Anthropic*, paste the key, chat
   with Claude; the *Cloud* mark shows in the header (details: *Cloud endpoints (P5)*, item 1).

## Memory estimates (P1)
On the Windows PC (RX 9070 XT, 16 GB), with `gemma-4-12B-coder…heretic`:
1. Browse at 65,536 tokens: Q8_0 says it fits (or offers a little less memory if less than
   about 15 GB is free on the card), never "Too large".
2. *Settings for this model*: Automatic reaches 32,768 tokens (green); 65,536 is yellow for
   Q8_0 and green for Q6_K and Q4_K_M.
3. Load Q6_K at 65,536 tokens and look at the log: the conversation memory on the graphics
   card is about 1,024 + 480 MiB (about 820 MiB with *Save memory*).
4. *Settings for this model*: the computer's bar shows "Saved copies of long chats (up to
   … GB)". With 7 GB or more of the computer's memory free that is about 2.5 GB (8 copies;
   about 1.3 GB with *Save memory*); with less free it is fewer copies (at least one, 320
   MB), never a red mark.
5. During a long chat (15 or more turns or tool calls), llama-server's memory in Task
   Manager grows and then levels off at no more than that figure: these are the model
   runner's saved copies of the conversation.
6. After that long chat, *Reload*: the model comes back with the same memory size (the
   ready line still says about 24,000 words with Automatic), not 4,096 tokens.
7. If you can get the computer to about 3.2 GB free (open other programs), *Settings for
   this model* shows yellow with "Long chats will use about 0.3 GB more of the computer's
   memory, which may not have that much free.", and Automatic still picks 32,768 tokens.
   (Below about 3 GB it is red, because the model's own part in the computer's memory, the
   1 GB input layer, does not fit; that was so before.)
8. With *Fast attention* off, the estimate rises to about 3 GB of conversation memory at
   65,536 tokens.

## Computer use (P3)
On the Windows PC:
1. Load a model that can see (for example Qwen 3.8 27B with its image file), switch on
   Computer use, keep harnessUI in front (also try it maximised), and ask: "Open Notepad and
   type hello."
   - The cards say harnessUI minimises its window while the input runs, if the window is in
     the way.
   - After Allow, harnessUI minimises when it covers the click or has the keyboard; the red
     border and the Stop bar stay on screen. A click elsewhere on the screen leaves it where
     it is.
   - Clicks and typing land in the app behind harnessUI.
   - For each card harnessUI comes back in front of Notepad, but typing on the keyboard
     still goes to Notepad, and its taskbar button flashes. Clicking *Allow* at once does
     nothing; a moment later it works.
   - At the end of the turn harnessUI comes back without taking the keyboard (type
     something: it goes to Notepad) and its taskbar button flashes.
2. With harnessUI focused and Notepad behind it, ask the model to type without clicking
   first. The card names Notepad and the text lands there, also with *Allow once*.
3. Press the stop hotkey (Ctrl+Alt+Backspace) or the Stop button while harnessUI is
   minimised: it comes back at once, with the keyboard, and no key stays held down.
4. Bring harnessUI back by hand during a turn: it stays up until the next input it is in
   the way of. Minimise it yourself before a turn: it stays minimised after the turn.
5. After Stop, Notepad's menu bar is not left highlighted (only Stop focuses harnessUI, and
   the app may tap Alt so Windows lets it take the focus). After a card or the end of a
   turn no Alt is tapped.
6. Typing into Start's search box always asks, and answering that card moves the focus to
   harnessUI, which closes the Start menu (as before 0.0.7). If a model gets stuck there,
   suggest Win+R or a shortcut instead.
7. Choose *Allow for this chat* for clicks in Notepad. That click itself steps aside as
   usual (you just answered its card), so harnessUI stays minimised: bring it back by hand,
   or send a new message once the turn has ended and harnessUI has come back. Then keep
   moving the mouse over harnessUI's window while the model clicks where harnessUI covers
   Notepad: the click is refused ("covered by this app's own window") and harnessUI stays.
   Let go of the mouse for two seconds: the next such click minimises harnessUI and lands in
   Notepad. Do the same with Allow all, and with typing while harnessUI has the keyboard
   (refused with "this app's own window has the keyboard focus").
8. Open the Allow all or Full access confirmation (header menu or Settings → Permissions)
   and leave it open while a chat runs under *Allow for this chat* or Allow all and
   minimises harnessUI. When a later card of that chat (an action that still asks) brings
   harnessUI back under your pointer, a click right away on the confirm button does nothing.
9. After a card, click Notepad: harnessUI goes behind it (it is not left always on top),
   and the Stop bar stays above harnessUI.
10. While the agent has minimised harnessUI, close the app the model used (so Windows hands
    the focus to another window), and another time change the display scale: harnessUI
    still comes back for the next card, at the end of the turn and on Stop.
11. Press Stop several times during the minimise animation: harnessUI always comes back,
    with the keyboard.
12. Answer a card and touch nothing until the next card or the end of the turn, and once
    after the model pressed Alt (for example `alt` or `alt+f`): harnessUI still comes back
    without the keyboard (type something: it goes to Notepad). Try it with harnessUI
    maximised too: it comes back maximised.
13. With the red border on screen, cards still take clicks after the short delay.
14. Let a task run into the step limit while harnessUI is minimised by the agent: when it
    comes back at the end of the turn, *Continue* under the chat and *Resume* in the Task tab
    do nothing for a moment, then work.

## Image files (P2)
On the Windows PC:
1. **Ministral 3 14B Reasoning Uncensored (mradermacher), with its broken image file:**
   - Load it: one retry on the same graphics card, no stepping down to the processor, and
     the header says "Loaded without image input: the image file next to this model couldn't
     be read." with *Learn more*.
   - *Learn more* opens Models → Installed; the row says the image file couldn't be read,
     with *Try it again*. As the model was downloaded before 0.0.7, the row asks Hugging Face
     once by itself and then offers *Download the image file again* (the copy here may differ
     from the repository's). Download it: it is checked, found to be the repository's file and
     not downloaded again; the next load fails the same way, and after that the row says
     "Its repository has no other image file."
   - Load it again: it goes straight through (the log shows no "failed to load multimodal
     model").
   - *Try it again*, then load it: the image file is tried (one retry and the same note).
   - Make a load fail some other way on the graphics card: the message no longer mentions
     the CPU backend.
2. **Qwen 3.8 27B Uncensored (HauhauCS), downloaded without its image file:**
   - Downloaded with this version: Models → Installed shows "Download the image file" with
     its size (the BF16 file, as that repository has no F16) at once. Downloaded before: it
     shows *Look for an image file*; click it and the button appears.
   - Download it (progress also under Downloads): the row says "It will be used the next
     time the model loads." (when it is the model loaded now: to reload it, with *Reload*).
     Then the model can see images, the Tools panel's Computer group can be switched on, and
     computer use works.
3. **A model with no image file:** put an `mmproj…gguf` next to it by hand and load it: it
   can now see images. A text-only model in a folder with other models' image files gets
   none and shows no note.
4. **Offline** (network cable out or Wi-Fi off): Models → Installed opens at once and says
   nothing about Hugging Face; *Look for an image file* says it couldn't be reached only when
   clicked.
5. **If a vision model runs out of memory while its image file loads** (a large one with *How
   much it can remember* set high): the app first puts less on the graphics card, then uses
   the processor; only if nothing else works does it load without the image file, with "Loaded
   without image input: there wasn't enough memory for the image file too." Installed then
   does not say the image file couldn't be read.
6. **A vision model whose image file is of another kind** (for example openbmb's MiniCPM-V
   2.6 GGUF, downloaded from Browse with its image file): it can see images, and Installed
   offers nothing for it.

## Computer use (P3), on the Mac
Repeat the Windows computer-use steps (first section) with TextEdit: typing reaches TextEdit
after answering a card, harnessUI's window comes to the front for a card even while
TextEdit is active (TextEdit stays the active app until you click the card), and the Dock
icon bounces for a card and at the end of the turn. The first click on a card shown in
front only makes harnessUI the active app (expected on a Mac); click again to answer.

## Cloud endpoints (P5) — needs your own API keys
1. **Anthropic (Claude):** Settings → Endpoints → Anthropic, paste the key. *Test* lists the
   models by name. In a chat the *Cloud* mark's tooltip names `api.anthropic.com`. With
   Thinking on, run a multi-step tool task in an approved folder: no error about thinking,
   and the Thinking area fills. Send a second message in the same chat: it should report
   many cached tokens (the earlier turn's thinking goes back unchanged). Leave an approval
   card waiting for more than five minutes, then allow it: the tools and system prompt
   should still come from the cache. Attach a picture. Try a wrong key: the message is plain
   and never shows the key. *Edit* shows "at most 200,000 tokens" for Automatic; choose
   1,000,000 and see the note about credit.
2. **Claude Opus 5.5:** the header's switch says *On*/*Low*; with *Low* a short question is
   answered quickly, and a long chat's automatic summary still works (not "Could not
   summarise"). Run two chats with the same model side by side, each with tools and
   thinking on: each keeps working turn after turn (no error about thinking or signatures).
   In one chat attach more than 20 pictures over several messages, one of them larger than
   3,000 pixels a side: the chat keeps answering, and Claude mentions that a picture was
   left out instead of the request failing. With exactly 21 pictures, two of them larger than
   3,000 pixels a side, only the older of the two is left out. With *Subagents* on, ask for
   a task the model hands to a helper: the helper and the chat both keep working with
   thinking on (no error about thinking or signatures).
3. **OpenAI:** only chat models are listed (no *-pro*, Codex or deep research models). A tool
   turn on a GPT-5-family model and on a 4.1-type model works (no error about `max_tokens` or
   temperature). Try a tool that returns a screenshot.
4. **OpenRouter:** models listed with names and sizes; a tool turn on a Claude model through
   OpenRouter and on an open model.
5. **abliteration.ai:** *Test*, then a chat that uses tools.
6. **All:** an `http://` internet address with a key is refused; Ollama at your network's IP
   address (for example `http://192.168.1.20:11434`) over `http://` with a key works and
   shows no *Cloud* mark, while the same server by name (`http://desktop.local:11434`) shows
   the mark and needs https for a key. Edit the OpenAI endpoint and switch it to
   OpenRouter: Save asks for the OpenRouter key instead of sending the OpenAI one.

## Downloads and Installed (P6b)
1. Download a vision model (model + image file). Pause it while the image file downloads:
   the row says paused and offers *Resume*; *Resume* continues only the image file (the
   finished model file is not downloaded again). Close and reopen harnessUI while paused:
   it stays paused.
2. Press *Download* twice quickly on Browse: only one download starts. Start the same model
   again from Browse while it downloads: it follows the running one.
3. Cancel a download after its first file finished: the row says "cancelling…" for a moment,
   then the finished file is deleted (unless an installed model uses it).
4. Copy a model file you have into the models folder at the place its download would use
   (`<models folder>\<owner>\<repo>\<file>`), then download that model from Browse: the
   row says "checking the file…" while its bar fills (no time left is shown), and the file
   is not downloaded again. For a vision model, cancel while its image file downloads: the
   file you copied in stays.
5. Delete a model's folder in Explorer, then remove it from Installed: the row goes away.
6. Delete a vision model you downloaded: the question counts the model and its image file
   ("This removes … GB from disk"), and both go. An image file you put next to a model
   yourself stays when that model is deleted.
7. With a reply waiting for your approval, reload the window (Ctrl+R): the card comes back
   and answering it works.

## Website capture (P7)
1. In a new chat open the Tools panel and check *browser_capture_site* is on. If you had
   changed tools before, the update switched it on once in each saved choice where every
   other Browser tool was on; where you had switched some Browser tools off it starts off
   (switch it on there). Switch it off in a chat and restart: it stays off.
2. Ask: "Capture https://gsap.com into my projects folder, then rebuild its hero with my own
   text." Read the card ("Save a copy of how gsap.com looks into …"), allow it.
3. Open the saved `index.html` from disk: it looks like the site and runs no scripts;
   compare it with the pictures in `screenshots/`.
4. Ask "make a site that looks like stripe.com": the capture tool is offered.
5. On Windows: a long folder path, and opening the copy in Edge.

## Tools, call guide, repeat limit (P4)
On the Windows PC, with a local model and the terminal on:
1. Ask "run `cd client && npm install`" (any folder): the failed result ends with one line
   saying Windows PowerShell 5.1 has no `&&` and to use `;` or
   `if ($LASTEXITCODE -eq 0) { … }`. Try `rm -rf build` and `curl https://example.com/?a=1&b=2`
   too: each ends with its own one-line hint, and the exit code shown is the command's own.
2. *Context* tab: the system message has a `<tool_list>` of every tool that is on and a
   `<tool_call_guide>` that says Windows PowerShell 5.1 (on the Mac: zsh). Send two more
   messages that bring in different tools: both blocks stay the same (only the tool
   definitions change).
3. With Computer use on, ask "open Notepad and type hello" (or "open Google Chrome", "open
   Microsoft Excel"): after the reply the Tools panel marks all seven Computer tools as shown
   in that chat. In a new chat, also with Computer use on, ask "how do I open excel files in
   python": they are not shown.
4. *Settings → General → Same call in a row*: set 3, then ask a model to read the same file
   over and over: it is warned on the second identical call and the turn stops at the third.
5. With *Subagents* on, ask for a helper: the helper's first request is quick to start (the
   model server reuses the chat's prompt; llama-server's log shows most of the prompt
   cached).
