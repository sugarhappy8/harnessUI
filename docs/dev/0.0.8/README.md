# 0.0.8 plan

0.0.8 makes harnessUI update itself. The owner's words, 2026-11-16, kept as they wrote them:

1. "id like to make harnessUI one universal installer that gets updates through the web. also
   add the installer as the download link for the website."
2. "please start building what i told u i want. it shouldnt be a installer from 0.0.7 it
   should just be one installer in my opinion ngl, its fine if people need to install a brand
   new app as long as from now on all updates will appear as a little box in the app saying
   that an update is ready and if the user wants to update the app."

This changes two points of [`docs/SPEC.md`](../../SPEC.md): "an opt-in update check" (under
Non-negotiable) and "Updates: opt-in check only" (under Packaging and release).

## Decisions

1. 0.0.8 is the first version that updates itself. People with 0.0.7 or older install it
   once by hand; from then on every update arrives in the app.
2. "One installer" is one Download button on harnessui.site that offers each visitor the
   file for their computer first, from the latest GitHub release. One file cannot install on
   both Windows and macOS; the Mac disk image already works on Apple silicon and Intel.
3. Every release also carries copies of its installers under names that never change,
   `harnessUI-setup.exe` and `harnessUI-universal.dmg`, so
   `https://github.com/sugarhappy8/harnessUI/releases/latest/download/<name>` always gives
   the newest one. The website and the README link there.
4. The automatic check is on by default (an "off" saved by an earlier version is kept):
   shortly after start, then every 6 hours. A newer version is downloaded and its signature
   verified in the background. Then a small box says "Update ready" and which version is
   ready to install, with *Restart and update*, *Later* and *What's new*. *Later* hides the
   box until the next start; a newer version shows it again. *Settings → General → Updates*
   has the switch, *Check now*, a status line and *Restart and update*. Installing stops the
   model and the tools, installs the file already downloaded and opens the new version.
5. Every release needs the updater key. Its public half is committed in
   `src-tauri/tauri.updater.conf.json`; the owner makes the key pair once on their own
   computer with `scripts/setup_updater.py`. Until then the release workflow stops before it
   builds anything.
6. The private key stays on the owner's computer and in the repository secrets; it never
   passes through a build session.
7. The builder creates no tags or releases: releasing is the owner's decision.

Where it is described: README.md ("Download" and "In-app updates"), ARCHITECTURE.md 13.6
and 16, SECURITY.md ("Updates"), CHANGELOG.md (0.0.8) and docs/MILESTONES.md (M8).
