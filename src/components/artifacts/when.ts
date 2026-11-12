/** When something happened, in a few words: "just now", "5 min ago", "3 h ago", a date. */
export function when(at: number, now: number = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return days === 1 ? "yesterday" : `${days} days ago`;
  return new Date(at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/** A file name made from a title: the characters file systems refuse are taken out. */
export function fileName(title: string, extension: string): string {
  const cleaned = title
    // Control characters and the characters Windows refuses in a name.
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f<>:"/\\|?*]+/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    // Windows refuses a name that ends in a dot or a space.
    .replace(/[. ]+$/, "")
    .slice(0, 80)
    .trim();
  return `${cleaned === "" ? "artifact" : cleaned}.${extension}`;
}
