/** Program version policies have the same interpretation in editors and compact setting summaries. */
export const PROGRAM_VERSIONS_KEY = 'agents.harness_versions';
export type VersionPolicy = 'shipped' | 'newest_checked' | 'newest' | 'exact';
export interface VersionChoice { use: VersionPolicy; version?: string }

export function versionChoice(value: unknown, kind: string): VersionChoice {
  const item = typeof value === 'object' && value !== null ? (value as Record<string, unknown>)[kind] : undefined;
  if (typeof item !== 'object' || item === null) return { use: 'shipped' };
  const { use, version } = item as Record<string, unknown>;
  return { use: use === 'exact' || use === 'newest' || use === 'newest_checked' ? use : 'shipped', ...(typeof version === 'string' ? { version } : {}) };
}
