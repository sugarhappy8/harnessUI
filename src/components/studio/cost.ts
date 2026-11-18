// What Studio's runs cost, as Studio shows it (0.1.0): dollars in one form everywhere (the
// top bar's spending, the picker's price hints, the Create button).

/** Dollars as Studio shows them: "$0.16"; more than nothing but less than a cent, "< $0.01". */
export function formatUsd(usd: number): string {
  if (usd > 0 && usd < 0.01) return "< $0.01";
  return `$${usd.toFixed(2)}`;
}
