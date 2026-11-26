export function memoryBelowUse(draft: unknown, usedBytes: number | null | undefined): boolean {
  if (usedBytes === null || usedBytes === undefined || typeof draft !== 'object' || draft === null) return false;
  const memory = (draft as Record<string, unknown>).memory_mb;
  return typeof memory === 'number' && memory > 0 && memory * 1024 * 1024 < usedBytes;
}
