/** One policy for returning readers. Live answers revalidate on show; release data lives for the page. */
import type { QueryKey } from '@tanstack/react-query';

export const READ_NOW = { staleTime: 0 } as const;
export const RELEASE_READ = { staleTime: Infinity, gcTime: Infinity } as const;
/** Directory contents change less often than status; repeated searches should reuse the same letters. */
export const DEFAULT_READ_FRESHNESS_MS = { directory: 60_000, lookup: 15_000, report: 60_000, status: 5_000 };
type Window = keyof typeof DEFAULT_READ_FRESHNESS_MS;
type Policy = Window | 'release' | 'session' | 'shown';

export const READ_FRESHNESS: readonly { prefixes: readonly (readonly string[])[]; policy: Policy }[] = [
  { prefixes: [['settings-registry'], ['permissions'], ['harness-kinds'], ['drivers'], ['capabilities'], ['notifiers']], policy: 'release' },
  { prefixes: [['shell-bootstrap'], ['me'], ['me-state'], ['orgs'], ['admin-mode'], ['favorites'], ['chat-states'], ['notification-prefs'], ['away', 'summary']], policy: 'session' },
  { prefixes: [['thread-shared-tree'], ['thread-shared'], ['document-links'], ['people'], ['org-groups'], ['recents']], policy: 'directory' },
  { prefixes: [['names'], ['search'], ['search-preview'], ['holder-search'], ['people-search']], policy: 'lookup' },
  { prefixes: [['usage', 'health'], ['usage', 'limits'], ['usage', 'caps'], ['usage', 'mine'], ['usage', 'waiting'], ['provider-health']], policy: 'status' },
  { prefixes: [['usage'], ['storage'], ['meta']], policy: 'report' },
];

let settings: () => Record<string, unknown> = () => ({});
export function readFreshnessSettings(source: () => Record<string, unknown>): void { settings = source; }

export function readFreshness(key: QueryKey): number {
  const policy = READ_FRESHNESS.find(({ prefixes }) => prefixes.some((prefix) => prefix.every((part, index) => key[index] === part)))?.policy ?? 'shown';
  if (policy === 'release' || policy === 'session') return Infinity;
  if (policy === 'shown') return 0;
  const own = settings()[policy];
  return typeof own === 'number' && Number.isSafeInteger(own) && own >= 0 ? own : DEFAULT_READ_FRESHNESS_MS[policy];
}
