# 0.0.7: what only the owner's computers can show

Each package adds what it could not check here (no Windows PC or Mac in the build machine).

## Memory estimates (P1)
On the Windows PC (RX 9070 XT, 16 GB), with `gemma-4-12B-coder…heretic`:
1. Browse at 65,536 tokens: Q8_0 says it fits (or offers a little less memory if less than
   about 15 GB is free on the card), never "Too large".
2. *Settings for this model*: Automatic reaches 32,768 tokens (green); 65,536 is yellow for
   Q8_0 and green for Q6_K and Q4_K_M.
3. Load Q6_K at 65,536 tokens and look at the log: the conversation memory on the graphics
   card is about 1,024 + 480 MiB (about 820 MiB with *Save memory*).
4. During a long chat (15 or more turns or tool calls), llama-server's memory in Task
   Manager grows by about 2.5 GB (about 1.3 GB with *Save memory*) and then levels off:
   these are the model runner's saved copies of the conversation, now at most 8.
5. With *Fast attention* off, the estimate rises to about 3 GB of conversation memory at
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
7. Choose *Allow for this chat* for clicks in Notepad, then keep moving the mouse over
   harnessUI's window while the model clicks where harnessUI covers Notepad: the click is
   refused ("covered by this app's own window") and harnessUI stays. Let go of the mouse
   for two seconds: the next such click minimises harnessUI and lands in Notepad. Do the
   same with Allow all and with typing while harnessUI has the keyboard. Once harnessUI is
   back (you brought it back), it is minimised again only after 1.5 s without input.
8. Run two chats. While harnessUI is minimised for one of them, let a card of the other
   chat wait for more than a second; when harnessUI comes back, a click right away on
   *Allow* does nothing, and a moment later it works.
9. Open the Allow all or Full access confirmation (header menu or Settings → Permissions)
   and leave it open, with a chat running under *Allow for this chat* or Allow all that
   minimises harnessUI. When a card brings harnessUI back under your pointer, a click right
   away on the confirm button does nothing.
10. After a card, click Notepad: harnessUI goes behind it (it is not left always on top),
    and the Stop bar stays above harnessUI.
11. While the agent has minimised harnessUI, close the app the model used (so Windows hands
    the focus to another window), and another time change the display scale: harnessUI
    still comes back for the next card, at the end of the turn and on Stop.
12. Press Stop several times during the minimise animation: harnessUI always comes back,
    with the keyboard.
13. Answer a card and touch nothing until the next card or the end of the turn, and once
    after the model pressed Alt (for example `alt` or `alt+f`): harnessUI still comes back
    without the keyboard (type something: it goes to Notepad). Try it with harnessUI
    maximised too: it comes back maximised.
14. With the red border on screen, cards still take clicks after the short delay.

## Image files (P2)
On the Windows PC:
1. **Ministral 3 14B Reasoning Uncensored (mradermacher), with its broken image file:**
   - Load it: one retry on the same graphics card, no stepping down to the processor, and
     the header says "Loaded without image input: the image file next to this model couldn't
     be read." with *Learn more*.
   - *Learn more* opens Models → Installed; the row says the image file couldn't be read and
     offers another one, or says the repository has no other one.
   - Load it again: it goes straight through (the log shows no "failed to load multimodal
     model").
   - Make a load fail some other way on the graphics card: the message no longer mentions
     the CPU backend.
2. **Qwen 3.8 27B Uncensored (HauhauCS), downloaded without its image file:**
   - Models → Installed shows "Download the image file" with its size (the BF16 file, as
     that repository has no F16).
   - Download it (progress also under Downloads), then *Reload*: the model can see images,
     the Tools panel's Computer group can be switched on, and computer use works.
3. **A model with no image file:** put an `mmproj…gguf` next to it by hand and load it: it
   can now see images.

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
   summarise").
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
3. Cancel a download after its first file finished: the finished file is deleted (unless
   an installed model uses it).
4. Delete a model's folder in Explorer, then remove it from Installed: the row goes away.
5. With a reply waiting for your approval, reload the window (Ctrl+R): the card comes back
   and answering it works.

## Website capture (P7)
1. In a new chat open the Tools panel and check *browser_capture_site* is on (if you have
   ever changed tools, new chats reuse that choice and a new tool may start off).
2. Ask: "Capture https://gsap.com into my projects folder, then rebuild its hero with my own
   text." Read the card ("Save a copy of how gsap.com looks into …"), allow it.
3. Open the saved `index.html` from disk: it looks like the site and runs no scripts;
   compare it with the pictures in `screenshots/`.
4. Ask "make a site that looks like stripe.com": the capture tool is offered.
5. On Windows: a long folder path, and opening the copy in Edge.
