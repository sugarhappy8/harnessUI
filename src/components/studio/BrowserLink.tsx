import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ErrorInfo } from "../../lib/types";
import { DOWNLOAD_LOCAL } from "../../lib/words";
import { toErrorInfo } from "../../store/app";

/** A model's page on Hugging Face. */
export function modelPage(repo: string): string {
  return `https://huggingface.co/${repo.split("/").map(encodeURIComponent).join("/")}`;
}

/** A link that opens in the browser, never in the app's window; a failure says where to go. */
export function BrowserLink(props: { href: string; title?: string; children: ReactNode; onError(e: ErrorInfo): void }) {
  const { href, onError } = props;
  return (
    <a
      href={href}
      title={props.title}
      className="text-accent underline"
      onClick={(e) => {
        e.preventDefault();
        openUrl(href).catch((err: unknown) =>
          onError({ ...toErrorInfo(err), message: DOWNLOAD_LOCAL.notOpened, nextStep: DOWNLOAD_LOCAL.goTo(href) }),
        );
      }}
    >
      {props.children}
    </a>
  );
}
