; NSIS installer hooks, wired in through bundle.windows.nsis.installerHooks in
; tauri.conf.json (ARCHITECTURE.md 16).
;
; Before any file is copied, stop a stray llama-server.exe that runs from the install
; folder. One can be left behind when the app ended without stopping its local model
; server; it keeps the runtime's files open, so copying the new ones over them would fail.
; Only processes whose program file is under $INSTDIR are stopped: a llama-server you run
; yourself, and the backend packs in the app's data folder, are left alone. A running
; harnissUI is closed by Tauri's own check right after this hook, and its Job Object ends
; the servers it started.
;
; The installer is a 32-bit program, so the PowerShell it starts is 32-bit too, and
; Get-Process cannot read the path of a 64-bit process from there; WMI (Win32_Process)
; reports ExecutablePath across bitness. NSIS expands $INSTDIR; $$ is a literal $, so
; PowerShell sees $_. Failing to stop a process does not stop the install. An install path
; containing a single quote breaks the PowerShell string, and then nothing is stopped.

!macro NSIS_HOOK_PREINSTALL
  Push $0
  nsExec::Exec /TIMEOUT=20000 `powershell.exe -NoProfile -NonInteractive -Command "Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object { $$_.Name -eq 'llama-server.exe' -and $$_.ExecutablePath -and $$_.ExecutablePath.StartsWith('$INSTDIR\', [StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { Stop-Process -Id $$_.ProcessId -Force -ErrorAction SilentlyContinue; Wait-Process -Id $$_.ProcessId -Timeout 5 -ErrorAction SilentlyContinue }"`
  Pop $0 ; nsExec's result
  Pop $0
!macroend
