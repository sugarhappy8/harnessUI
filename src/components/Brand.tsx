/** The app's mark: the two overlapping windows of the app icon on the accent gradient. */
export function BrandMark(props: { size?: number; glow?: boolean }) {
  const size = props.size ?? 24;
  return (
    <span
      aria-hidden
      className={"brand-gradient inline-flex shrink-0 items-center justify-center " + (props.glow ? "shadow-float" : "")}
      style={{ width: size, height: size, borderRadius: size * 0.28, color: "#fff" }}
    >
      <svg width={size * 0.62} height={size * 0.62} viewBox="0 0 24 24" fill="none">
        <rect x="2.5" y="4" width="13" height="10.5" rx="3" fill="currentColor" />
        <rect x="9.5" y="10" width="12" height="9.5" rx="3" fill="currentColor" opacity="0.62" />
      </svg>
    </span>
  );
}
