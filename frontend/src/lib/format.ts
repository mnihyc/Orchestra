/** Numbers, bytes and token counts.
 *
 * Numbers are written by `Intl` for the reader's language. Sizes are binary, with the units that say
 * so (KiB, MiB, GiB); the size of a disk and what is free on it are decimal (GB), as every storage report gives them.
 * Token counts are abbreviated once they pass ten thousand ("12.4k").
 */

const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'] as const;
const BYTES_PER_STEP = 1024;
const DISK_UNITS = ['B', 'kB', 'MB', 'GB', 'TB', 'PB'] as const;
const DISK_BYTES_PER_STEP = 1000;
const DISK_BYTES_PER_GB = 1_000_000_000;

/** Above this a count of tokens is abbreviated: a longer figure is no longer read digit by digit. */
const TOKENS_ABBREVIATED_ABOVE = 10_000;

let ordinary: { locale: string; formatter: Intl.NumberFormat } | undefined;

/** A number with the grouping and the decimal mark of the language. */
export function formatNumber(value: number, locale: string, options?: Intl.NumberFormatOptions): string {
  if (options !== undefined) return new Intl.NumberFormat(locale, options).format(value);
  if (ordinary?.locale !== locale) ordinary = { locale, formatter: new Intl.NumberFormat(locale) };
  return ordinary.formatter.format(value);
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

/** The size of a disk or what is free on it, in decimal units: the gigabytes of 10^9 bytes that the node, the connector and
 * every driver report a storage in, as `df -H` says them, so that one disk reads the same on every page. One decimal. */
export function formatDiskBytes(bytes: number, locale: string): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= DISK_BYTES_PER_STEP && unit < DISK_UNITS.length - 1) {
    value /= DISK_BYTES_PER_STEP;
    unit += 1;
  }
  const figure = formatNumber(value, locale, { maximumFractionDigits: unit === 0 ? 0 : 1, minimumFractionDigits: 0 });
  return `${figure} ${DISK_UNITS[unit] ?? 'B'}`;
}

/** A storage's size or free space as a host or a machine reports it, in whole decimal gigabytes. */
export function formatStorageGb(gigabytes: number, locale: string): string {
  return formatDiskBytes(gigabytes * DISK_BYTES_PER_GB, locale);
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
