# 0.0.7: what only the owner's computers can show

Each package adds what it could not check here (no Windows PC or Mac in the build machine).

## Computer use (P3)
On the Windows PC:
1. Load a model that can see (for example Qwen 3.8 27B with its image file), switch on
   Computer use, keep harnessUI in front (also try it maximised), and ask: "Open Notepad and
   type hello."
   - The cards say harnessUI will minimise itself while the input runs.
   - After Allow, harnessUI minimises; the red border and the Stop bar stay on screen.
   - Clicks and typing land in the app behind harnessUI.
   - harnessUI comes back, in front, for each card and at the end of the turn.
2. With harnessUI focused and Notepad behind it, ask the model to type without clicking
   first. The card names Notepad and the text lands there, also with *Allow once*.
3. Press the stop hotkey (Ctrl+Alt+Backspace) or the Stop button while harnessUI is
   minimised: it comes back at once and no key stays held down.
4. Bring harnessUI back by hand during a turn: it stays up until the next approved input.
   Minimise it yourself before a turn: it stays minimised after the turn.
5. When harnessUI comes back, Notepad's menu bar is not left highlighted (the app taps Alt
   so Windows lets it take the focus).
6. Typing into Start's search box always asks, and answering that card moves the focus to
   harnessUI, which closes the Start menu (as before 0.0.7). If a model gets stuck there,
   suggest Win+R or a shortcut instead.

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
after answering a card, and harnessUI's window comes to the front for a card even while
TextEdit is active.
