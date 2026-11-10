import { useEffect, useState } from "react";
import { Dialog } from "radix-ui";
import { X } from "lucide-react";
import { applyTheme, loadTheme, type ThemeChoice } from "../lib/theme";
import { appInfo, isTauri, type AppInfo } from "../lib/ipc";
import { APP_NAME } from "../lib/app";

const THEMES: { value: ThemeChoice; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

export function SettingsDialog(props: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [theme, setTheme] = useState<ThemeChoice>(loadTheme);
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    if (props.open && isTauri() && !info) {
      appInfo()
        .then(setInfo)
        .catch(() => setInfo(null));
    }
  }, [props.open, info]);

  function choose(value: ThemeChoice) {
    setTheme(value);
    applyTheme(value);
  }

  return (
    <Dialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 w-[28rem] max-w-[90vw] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-bg p-5 shadow-xl">
          <div className="mb-4 flex items-center justify-between">
            <Dialog.Title className="text-base font-semibold">Settings</Dialog.Title>
            <Dialog.Close
              className="rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg"
              aria-label="Close settings"
            >
              <X size={16} aria-hidden />
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">
            Appearance and information about this app.
          </Dialog.Description>

          <fieldset className="mb-5">
            <legend className="mb-2 text-sm font-medium">Theme</legend>
            <div className="flex gap-2" role="radiogroup" aria-label="Theme">
              {THEMES.map((t) => (
                <button
                  key={t.value}
                  type="button"
                  role="radio"
                  aria-checked={theme === t.value}
                  onClick={() => choose(t.value)}
                  className={
                    "rounded-md border px-3 py-1.5 text-sm " +
                    (theme === t.value
                      ? "border-accent bg-accent text-accent-fg"
                      : "border-border hover:bg-bg-muted")
                  }
                >
                  {t.label}
                </button>
              ))}
            </div>
          </fieldset>

          <section aria-label="About">
            <h2 className="mb-1 text-sm font-medium">About</h2>
            <p className="text-sm text-fg-muted">
              {info
                ? `${info.name} ${info.version} · ${info.os} ${info.arch}`
                : APP_NAME}
            </p>
            <p className="mt-1 text-xs text-fg-muted">
              Runs locally. No account, no telemetry. Licenses for bundled software are in
              THIRD-PARTY-NOTICES.md next to the app.
            </p>
          </section>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
