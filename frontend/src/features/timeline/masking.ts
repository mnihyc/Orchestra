/** Public native fields cannot be used as addresses after capture masking changed their text. */
export function nativeFieldMasked(payload: { masked_fields?: readonly string[] }, path: string): boolean {
  return payload.masked_fields?.some((field) => field === '' || field === path || path.startsWith(`${field}/`) || field.startsWith(`${path}/`)) === true;
}

/** A displayed search target can come from the action or from a returned result. */
export function nativeSearchAddressMasked(payload: { masked_fields?: readonly string[] }, index: number): boolean {
  return nativeFieldMasked(payload, index < 0 ? '/tool/action/url' : `/tool/results/${index}/url`);
}
