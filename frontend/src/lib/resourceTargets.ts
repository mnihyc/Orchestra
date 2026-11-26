/** Typed choice identity retains exact placement/configuration and extension fields, without authority. */
import type { ResourceTarget } from '@/generated/api-types';

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value !== null && typeof value === 'object') return Object.fromEntries(Object.entries(value)
    .sort(([one], [other]) => one < other ? -1 : one > other ? 1 : 0).map(([key, child]) => [key, canonical(child)]));
  return value;
}
export function resourceTargetIdentity(target: ResourceTarget): string { return JSON.stringify(canonical(target)); }
