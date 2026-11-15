/** Native permission-review facts shared by the row title and its factual card. */
import type { TFunction } from 'i18next';

export function approvalReview(value: unknown) {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const row = value as Record<string, unknown>;
  if (typeof row.model !== 'string' || typeof row.outcome !== 'string') return null;
  return { model: row.model, outcome: row.outcome, reason: typeof row.reason === 'string' ? row.reason : null, reported: row.reported === true, action: row.action, usage: row.usage };
}

/** Whether the review ended in a way that is a failure to give leave: what the harness's own call marks as an error. */
export function reviewFailed(outcome: string): boolean {
  return ['denied', 'failed', 'timedOut', 'timed_out', 'aborted'].includes(outcome);
}

export function reviewOutcome(outcome: string, t: TFunction): string {
  switch (outcome) {
    case 'inProgress': case 'in_progress': return t('timeline.permissionReview.status.reviewing');
    case 'approved': return t('timeline.permissionReview.status.approved');
    case 'denied': return t('timeline.permissionReview.status.denied');
    case 'timedOut': case 'timed_out': return t('timeline.permissionReview.status.timedOut');
    case 'aborted': return t('timeline.permissionReview.status.aborted');
    case 'failed': return t('timeline.permissionReview.status.failed');
    default: return t('timeline.permissionReview.status.other', { outcome });
  }
}
