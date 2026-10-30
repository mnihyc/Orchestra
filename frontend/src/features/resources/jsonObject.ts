/** Object configuration is checked before it is sent, keeping invalid text in the form. */
export function readObject(text: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(text);
    return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null;
  } catch { return null; }
}

/** Only the descriptor members the server marks as overridden are written back as overrides. */
export function descriptorOverrides(descriptor: unknown, keys: readonly string[]): Record<string, unknown> {
  const source = typeof descriptor === 'object' && descriptor !== null && !Array.isArray(descriptor) ? descriptor as Record<string, unknown> : {};
  return Object.fromEntries(keys.filter((key) => Object.hasOwn(source, key)).map((key) => [key, source[key]]));
}
