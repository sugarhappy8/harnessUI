import { useApp } from "../store/app";

/** Local model state: loading progress, backend and device, context size. */
export function RuntimeIndicator() {
  const runtime = useApp((s) => s.runtime);
  const detail = useApp((s) => s.detail);
  const isLocal = detail?.chat.modelRef?.startsWith("local:") ?? false;
  if (!runtime || !isLocal) return null;
  if (runtime.state === "starting") {
    const pct = runtime.progress !== null ? Math.round(runtime.progress * 100) : null;
    return (
      <div className="flex items-center gap-2 text-xs text-fg-muted" aria-live="polite">
        <span>
          Loading model{runtime.stage ? ` (${runtime.stage.replace("_", " ")})` : ""}
          {pct !== null ? ` ${pct}%` : "…"}
        </span>
        <div className="h-1.5 w-24 overflow-hidden rounded bg-bg-muted" aria-hidden>
          <div
            className="h-full bg-accent transition-all"
            style={{ width: `${pct ?? 15}%` }}
          />
        </div>
      </div>
    );
  }
  if (runtime.state === "ready") {
    const where = runtime.device ? `${runtime.backend} · ${runtime.device}` : (runtime.backend ?? "");
    const gpu = runtime.backend !== null && runtime.backend !== "cpu";
    const partial =
      gpu &&
      runtime.offloadedLayers !== null &&
      runtime.totalLayers !== null &&
      runtime.offloadedLayers < runtime.totalLayers;
    return (
      <span className="flex items-center gap-2 text-xs text-fg-muted" title={runtime.modelPath ?? undefined}>
        <span>
          {where}
          {runtime.nCtx ? ` · ${runtime.nCtx.toLocaleString()} context` : ""}
        </span>
        {partial && (
          <span
            className="text-warning"
            title="The rest runs on the CPU, which is much slower. A smaller context or a smaller quantization fits fully on the GPU."
          >
            {runtime.offloadedLayers} of {runtime.totalLayers} layers on the GPU
          </span>
        )}
        {runtime.slow && runtime.speedTps !== null && (
          <span
            className="text-warning"
            title="The model loaded on the GPU but generates very slowly. It may have spilled into system memory: use a smaller context or model, or try another backend in Settings → Local models."
          >
            Loaded but slow ({runtime.speedTps.toFixed(1)} tokens/s)
          </span>
        )}
      </span>
    );
  }
  if (runtime.state === "failed") return <span className="text-xs text-danger">Model failed to load</span>;
  return null;
}
