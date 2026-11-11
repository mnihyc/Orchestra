/** Audience facts from the existing native permission page; never an access decision. */
import type { Entry, Holder, PermissionPage } from '@/generated/api-types';
import { holderKey, holdersOf, rankOf, type Catalogue } from '@/features/access/model';

export interface AudienceHolder { key: string; holder: Holder; sends: boolean }

function audienceTokens(holders: readonly AudienceHolder[]): string[] {
  return holders.flatMap(({ key, sends }) => sends ? [`view:${key}`, `send:${key}`] : [`view:${key}`]);
}

/** Keep grant identities and sending reach, without retaining names or expanded membership. */
export function audienceDismissal(holders: readonly AudienceHolder[]): string {
  return JSON.stringify(audienceTokens(holders).sort());
}

/** A smaller audience stays acknowledged; a new holder or sender needs a new notice. */
export function audienceGrew(holders: readonly AudienceHolder[], dismissed: string | null): boolean {
  if (dismissed === null) return true;
  try {
    const value: unknown = JSON.parse(dismissed);
    if (!Array.isArray(value) || !value.every((token) => typeof token === 'string')) return true;
    const known = new Set<string>(value);
    return audienceTokens(holders).some((token) => !known.has(token));
  } catch { return true; }
}

function live(entry: Entry, now: number): boolean {
  if (entry.expires_at === null) return true;
  const end = Date.parse(entry.expires_at);
  // An unreadable timestamp cannot prove that an audience disappeared.
  return !Number.isFinite(end) || end > now;
}

/** Named groups and roles remain audiences even when their current membership is empty. */
export function threadAudience(page: PermissionPage, catalogue: Catalogue, owner: string, now: number): AudienceHolder[] {
  const rows = page.rows.filter((row) => row.permission !== 'access.own'
    && catalogue.defs.get(row.permission)?.on.includes('thread') === true).map((row) => {
    const inherited = row.inherited;
    const entries = inherited?.entries ?? [];
    return { ...row, entries: row.entries.filter((entry) => live(entry, now)),
      inherited: inherited === null ? null : { ...inherited,
        holders: entries.length === 0 ? inherited.holders : entries.filter((entry) => live(entry, now)).map((entry) => entry.holder),
      } };
  });
  const found = new Map<string, AudienceHolder>();
  for (const named of holdersOf(rows).values()) {
    found.set(named.key, { key: named.key, holder: named.holder,
      sends: named.holds.some((hold) => {
        const definition = catalogue.defs.get(hold.permission);
        return definition?.area === 'thread' && (rankOf(definition) ?? -1) >= 1;
      }) });
  }
  for (const entry of page.owners.filter((entry) => live(entry, now))) {
    const key = holderKey(entry.holder);
    found.set(key, { key, holder: entry.holder, sends: true });
  }
  return [...found.values()].filter(({ holder }) => holder.kind !== 'user' || holder.user.id !== owner);
}

/** Use the existing visible clock only while an actual grant has an expiry. */
export function audienceExpires(page: PermissionPage): boolean {
  return [...page.owners, ...page.rows.flatMap((row) => [...row.entries, ...(row.inherited?.entries ?? [])])]
    .some((entry) => entry.expires_at !== null);
}
