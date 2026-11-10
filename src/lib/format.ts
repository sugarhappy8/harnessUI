/** Decimal gigabytes / megabytes, the units Hugging Face shows file sizes in. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "—";
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`;
  if (bytes >= 1e3) return `${(bytes / 1e3).toFixed(0)} KB`;
  return `${bytes} B`;
}

export function formatCount(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`;
  return String(n);
}

export function formatParams(n: number | null): string | null {
  if (!n) return null;
  if (n >= 1e9) return `${(n / 1e9).toFixed(n >= 1e10 ? 0 : 1)}B params`;
  return `${(n / 1e6).toFixed(0)}M params`;
}

/** "3 min left" style estimate. */
export function formatEta(remainingBytes: number, bytesPerSecond: number | null): string | null {
  if (!bytesPerSecond || bytesPerSecond <= 0 || remainingBytes <= 0) return null;
  const s = remainingBytes / bytesPerSecond;
  if (s < 60) return `${Math.ceil(s)} s left`;
  if (s < 3600) return `${Math.ceil(s / 60)} min left`;
  return `${(s / 3600).toFixed(1)} h left`;
}
