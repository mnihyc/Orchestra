/** Numbered turn links accept only a positive safe ordinal supplied by the ledger. */
export function turnOrdinal(value: unknown): number | undefined {
  if (typeof value !== 'string' && typeof value !== 'number') return undefined;
  if (typeof value === 'string' && !/^[1-9]\d*$/.test(value)) return undefined;
  const ordinal = Number(value);
  return Number.isSafeInteger(ordinal) && ordinal > 0 ? ordinal : undefined;
}
