import { useEffect, useRef, useState } from "react";
import { ArrowLeft, Check, CircleSlash, Download, ExternalLink, Lock } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo, RepoView, VariantView } from "../../lib/types";
import { licenseWords } from "../../lib/abilities";
import { formatBytes, formatParams } from "../../lib/format";
import { CANT_OPEN, tokensWithWords } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { COMPARE_FULL, COMPARE_LIMIT, useAddToComparison, useCompare } from "../../store/compare";
import { ErrorBanner } from "../ErrorBanner";
import { FitBadge } from "../FitBadge";
import { SafeLink } from "../Markdown";
import { Badge } from "./Badge";
import { CantOpenBox } from "./CantOpen";
import { AddToComparison, Outside, shown } from "./Compare";

/** The most of a model card shown (the backend reads at most 256 KB of it). */
const CARD_CHARS = 262_144;

/**
 * Add to comparison for one file of the repository (the file picked on this page). The
 * page is remembered so the Compare tab's Change menu needs no new request.
 */
function CompareVariant(props: { view: RepoView; variant: VariantView }) {
  const file = props.variant.files[0]?.path ?? null;
  const compare = useAddToComparison({ repo: props.view.id, file });
  const rememberRepo = useCompare((s) => s.rememberRepo);
  if (file === null) return null;
  return (
    <AddToComparison
      added={compare.added}
      full={compare.full}
      onAdd={() => {
        rememberRepo(props.view, null);
        compare.add();
      }}
      onRemove={compare.remove}
      name={shown(props.variant.label, 80)}
    />
  );
}

/**
 * One Hugging Face repository: its files with a fit badge each, and Download. For a model this
 * version cannot open yet (the model runner it comes with does not know its kind) the page says
 * that once, in a box, and nothing contradicts it: no memory verdict, no image-file offer, and
 * words where Download would be, never a dimmed button.
 */
export function RepoDetail(props: { repoId: string; onBack: () => void; onStarted: () => void }) {
  const [view, setView] = useState<RepoView | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [withProjector, setWithProjector] = useState(true);
  const [showCard, setShowCard] = useState(false);
  // [0.0.7 P6] While a download starts (the app asks Hugging Face first), Download is off: a
  // second click started a second download of the same files, and the two spoiled each other.
  const [starting, setStarting] = useState(false);
  const startingRef = useRef(false);
  const compareFull = useCompare((s) => s.entries.length >= COMPARE_LIMIT);

  useEffect(() => {
    let alive = true;
    ipc
      .hubRepo(props.repoId, null)
      .then((v) => alive && setView(v))
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, [props.repoId]);

  const cantOpen = view?.architectureSupported === false;
  // Picture, video, speech, an early layout: no update would change it, so the words say what
  // it is instead.
  const noUpdate = cantOpen && view?.cantOpenWhy ? CANT_OPEN[view.cantOpenWhy] : null;

  async function download(v: VariantView) {
    if (!view || cantOpen || startingRef.current) return;
    startingRef.current = true;
    setStarting(true);
    try {
      await ipc.startDownload(view.id, view.commit, v.label, withProjector && !!view.projector);
      props.onStarted();
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      startingRef.current = false;
      setStarting(false);
    }
  }

  const hfUrl = `https://huggingface.co/${props.repoId}`;
  return (
    <div className="space-y-3">
      <button type="button" onClick={props.onBack} className="flex items-center gap-1 text-xs text-fg-muted hover:text-fg">
        <ArrowLeft size={12} aria-hidden /> Back to results
      </button>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="truncate text-base font-semibold" title={props.repoId}>
            {props.repoId}
          </h3>
          {view && (
            <div className="mt-1 flex flex-wrap gap-1">
              {view.architecture && (
                <Badge tone={cantOpen ? "danger" : undefined}>
                  {cantOpen && <CircleSlash size={10} className="mr-0.5 inline" aria-hidden />}
                  <Outside text={view.architecture} cap={60} />
                </Badge>
              )}
              {formatParams(view.parameters) && <Badge>{formatParams(view.parameters)}</Badge>}
              {view.contextLength && <Badge>remembers up to {tokensWithWords(view.contextLength)}</Badge>}
              {view.templateTools && <Badge>tools</Badge>}
              {view.templateThinking && <Badge>thinking</Badge>}
              {view.projector && <Badge>vision</Badge>}
              {view.license && (
                <Badge>
                  <Outside text={licenseWords(view.license)} cap={60} />
                </Badge>
              )}
              {view.gated !== "no" && (
                <Badge title="You need a Hugging Face token and must accept the terms on the model page">
                  <Lock size={10} className="mr-0.5 inline" aria-hidden /> gated
                </Badge>
              )}
            </div>
          )}
        </div>
        {/* Through the destination-confirm dialog, like links in model output. */}
        <span className="flex shrink-0 items-center gap-1 text-xs">
          <SafeLink href={hfUrl}>Open on Hugging Face</SafeLink>
          <ExternalLink size={11} aria-hidden />
        </span>
      </div>
      {view && cantOpen && <CantOpenBox architecture={view.architecture} why={view.cantOpenWhy} />}
      {!view && !error && <p className="text-sm text-fg-muted">Loading files…</p>}
      {view && (
        <>
          {view.projector && !cantOpen && (
            <label className="flex items-center gap-2 text-xs">
              <input type="checkbox" checked={withProjector} onChange={(e) => setWithProjector(e.target.checked)} />
              Also download the image file ({formatBytes(view.projector.size)}) so the model can read images
            </label>
          )}
          {/* A model no update would open gets no memory line: "once this version can open it"
              would promise what is not true. */}
          {!noUpdate && (
            <p className="text-xs text-fg-muted">
              {cantOpen
                ? CANT_OPEN.memory
                : `Memory estimates assume the model remembers ${tokensWithWords(view.fitContext)}.`}
            </p>
          )}
          {compareFull && <p className="text-xs text-fg-muted">{COMPARE_FULL}</p>}
          <ul className="space-y-2">
            {view.variants.map((v) => (
              <li key={v.label} className="flex items-center gap-3 rounded-md border border-border px-3 py-2 text-sm">
                <div className="min-w-0 flex-1">
                  <p className="font-medium">
                    <Outside text={v.label} cap={80} />
                    <span className="ml-2 text-xs font-normal text-fg-muted">
                      {formatBytes(v.totalSize)}
                      {v.files.length > 1 ? ` · ${v.files.length} parts` : ""}
                    </span>
                  </p>
                  {v.fit && !cantOpen && <FitBadge fit={v.fit} />}
                  {!v.hasChecksum && (
                    <p className="text-xs text-fg-muted">No checksum is published for this file.</p>
                  )}
                </div>
                <CompareVariant view={view} variant={v} />
                {v.installed ? (
                  <span className="flex items-center gap-1 text-xs text-success">
                    <Check size={12} aria-hidden /> Installed
                  </span>
                ) : cantOpen ? (
                  <span title={noUpdate ? noUpdate.title : CANT_OPEN.title} className="text-xs text-fg-muted">
                    {noUpdate ? noUpdate.short : CANT_OPEN.short}
                  </span>
                ) : (
                  <button
                    type="button"
                    onClick={() => void download(v)}
                    disabled={starting}
                    className="flex items-center gap-1 rounded-md bg-accent px-3 py-1 text-xs text-accent-fg disabled:opacity-40"
                  >
                    <Download size={12} aria-hidden /> Download
                  </button>
                )}
              </li>
            ))}
            {view.variants.length === 0 && (
              <li className="text-sm text-fg-muted">This repository has no GGUF model files.</li>
            )}
          </ul>
          {view.incomplete.length > 0 && (
            <p className="text-xs text-fg-muted">
              Not shown because parts are missing: <Outside text={view.incomplete.join(", ")} cap={600} />
            </p>
          )}
          {view.card && (
            <div>
              <button type="button" className="text-xs underline" onClick={() => setShowCard((s) => !s)}>
                {showCard ? "Hide model card" : "Show model card"}
              </button>
              {showCard && (
                <pre className="mt-2 max-h-64 overflow-auto rounded-md bg-bg-muted p-3 text-xs whitespace-pre-wrap">
                  {shown(view.card, CARD_CHARS, true)}
                </pre>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}
