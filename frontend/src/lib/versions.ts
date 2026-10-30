import type { HarnessBuild } from '@/generated/api-types';

/** The controller marks download-integrity refusals separately from selectable compatibility failures. */
export function buildRefusal(build: HarnessBuild): string | null {
  const error = build.check?.error;
  return error?.startsWith('refused:') === true ? error : null;
}

/** Numerical release ordering avoids calling 0.99 newer than 0.164. */
export function compareVersions(a: string, b: string): number {
  const one = a.replace(/^v/u, '').split(/[.+-]/u).map((part) => Number(part) || 0);
  const other = b.replace(/^v/u, '').split(/[.+-]/u).map((part) => Number(part) || 0);
  for (let index = 0; index < Math.max(one.length, other.length); index += 1) {
    const difference = (one[index] ?? 0) - (other[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}
