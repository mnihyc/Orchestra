/** Numbers, bytes and token counts.
 *
 * Numbers are written by `Intl` for the reader's language. Sizes are binary, with the units that say
 * so (KiB, MiB, GiB). Token counts are abbreviated once they pass ten thousand ("12.4k").
 */

const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'] as const;
const BYTES_PER_STEP = 1024;

/** Above this a count of tokens is abbreviated: a longer figure is no longer read digit by digit. */
const TOKENS_ABBREVIATED_ABOVE = 10_000;

/** A number with the grouping and the decimal mark of the language. */
export function formatNumber(value: number, locale: string, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(locale, options).format(value);
}

/** A size in binary units, with one decimal where the figure is small enough to need it. */
export function formatBytes(bytes: number, locale: string): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= BYTES_PER_STEP && unit < BYTE_UNITS.length - 1) {
    value /= BYTES_PER_STEP;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  const figure = formatNumber(value, locale, { maximumFractionDigits: digits, minimumFractionDigits: 0 });
  return `${figure} ${BYTE_UNITS[unit] ?? 'B'}`;
}

/** A count of tokens: in full up to ten thousand, then in thousands and millions with one decimal. */
export function formatTokens(tokens: number, locale: string): string {
  if (tokens <= TOKENS_ABBREVIATED_ABOVE) return formatNumber(tokens, locale);
  const [value, suffix] = tokens >= 1_000_000 ? [tokens / 1_000_000, 'M'] : [tokens / 1_000, 'k'];
  return `${formatNumber(value, locale, { maximumFractionDigits: 1 })}${suffix}`;
}

/** A share of a whole, from a ratio between 0 and 1. */
export function formatPercent(ratio: number, locale: string): string {
  return formatNumber(ratio, locale, { style: 'percent', maximumFractionDigits: 0 });
}
