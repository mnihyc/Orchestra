/** Frozen holder-share reviews and revision-bound inverses of confirmed additions and updates. */
import { ApiError } from '@/api/client';
import { changePage, type AccessTarget } from '@/api/access';
import type { PageChange, PageChangeRow, PermissionPage, SharePreview } from '@/generated/api-types';

export interface ShareReview {
  target: AccessTarget;
  before: PermissionPage;
  change: PageChange;
  preview: SharePreview;
}
export interface ShareResult {
  review: ShareReview;
  state: 'queued' | 'applying' | 'succeeded' | 'skipped' | 'refused' | 'uncertain' | 'observed';
  after?: PermissionPage;
  observed?: PermissionPage;
  error?: unknown;
  undone?: boolean;
}

/** Touches only entries this successful share added or changed, against the returned page revision. */
export function inverseShare(before: PermissionPage, after: PermissionPage): PageChange {
  const rows: PageChangeRow[] = [];
  for (const row of after.rows) {
    const old = before.rows.find((one) => one.permission === row.permission && one.profile === row.profile);
    const remove = row.entries.filter((entry) => old?.entries.some((one) => one.id === entry.id) !== true).map((entry) => entry.id);
    const update = row.entries.flatMap((entry) => {
      const previous = old?.entries.find((one) => one.id === entry.id);
      return previous === undefined || previous.revision === entry.revision ? [] : [{ entry_id: entry.id, options: previous.options, expires_at: previous.expires_at, note: previous.note }];
    });
    if (remove.length > 0 || update.length > 0) rows.push({ permission: row.permission, ...(row.profile === 'any' ? {} : { profile: row.profile }), remove, update });
  }
  return { rows, expected_revision: after.revision };
}

/** A lost answer remains uncertain, even if the requested access is seen by a later read. */
export async function applyShares(reviews: readonly ShareReview[], update: (result: ShareResult) => void): Promise<ShareResult[]> {
  const results: ShareResult[] = [];
  for (const review of reviews) {
    let result: ShareResult;
    if (!review.preview.allowed || review.preview.revision !== review.change.expected_revision) result = { review, state: 'skipped' };
    else {
      update({ review, state: 'applying' });
      try { result = { review, state: 'succeeded', after: await changePage(review.target, review.change) }; }
      catch (error) { result = { review, state: error instanceof ApiError && error.status < 500 ? 'refused' : 'uncertain', error }; }
    }
    results.push(result); update(result);
  }
  return results;
}

export function canUndoShare(result: ShareResult): boolean {
  return result.state === 'succeeded' && result.undone !== true && result.after !== undefined && (inverseShare(result.review.before, result.after).rows?.length ?? 0) > 0;
}
