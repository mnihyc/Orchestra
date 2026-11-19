/** Pure rules of the source screens: when a recorded state can be put back, what a plan allows, why nothing was recorded, the observed facts of work, a pull request's address, and the one message line comments make.
 *
 * Nothing here invents a fact the server did not give: an unknown flag stays unknown (never "clean"),
 * a stale observation says so, and technical reasons are translated into the reader's words.
 */
import type { TFunction } from 'i18next';

import { ApiError } from '@/api/client';
import type { DirectoryWork, RestorePlan, ReviewComment, Snapshot, ThreadAttention, ThreadLink } from '@/generated/api-types';
import { graphemeCount } from '@/lib/grapheme';

/** Source writes obey the machine's policy; protocol codes never become the explanation. */
export function sourceFailure(t: TFunction, reason: string): string {
  return reason === 'workspace.remote_write_disabled' || reason === 'policy.remote_write_disabled'
    ? t('source.prepare.writeDisabled') : t('source.prepare.failureReason');
}

/** Whether a recorded state is one a whole directory can be put back to: complete, nothing missing. */
export function restorable(snapshot: Snapshot): boolean {
  return snapshot.state === 'complete' && snapshot.missing_count === 0;
}

/** Whether a plan may be confirmed: it writes something, keeps a complete undo state, and nothing stops it. */
export function confirmable(plan: RestorePlan): boolean {
  return plan.paths.length > 0 && plan.undo_available && plan.blockers.length === 0;
}

/** Why a turn's files cannot be put back as a whole, in the reader's words. */
export function unrecordedReason(t: TFunction, snapshot: Snapshot | null): string {
  if (snapshot === null) return t('source.restore.why.notRecorded');
  if (snapshot.state === 'expired') return t('source.restore.why.expired');
  if (snapshot.state === 'running') return t('source.restore.why.recording');
  switch (snapshot.unavailable_reason) {
    case 'live_tree':
      return t('source.restore.why.liveTree');
    case 'not_ready':
      return t('source.restore.why.notReady');
    case 'store_unavailable':
      return t('source.restore.why.storeUnavailable');
    case 'too_large':
      return t('source.restore.why.tooLarge');
    case 'off':
      return t('source.restore.why.recordingOff');
    case 'offline':
      return t('source.restore.why.offline');
    case null:
      return snapshot.state === 'complete' ? t('source.restore.why.missing', { count: snapshot.missing_count }) : t('source.restore.why.incomplete');
    default:
      return t('source.restore.why.other');
  }
}

/** Why a restore plan cannot be confirmed, in words, or nothing where another line of the plan says it already (a missing undo). Never the protocol code. */
export function restoreBlocker(t: TFunction, code: string): string | null {
  switch (code) {
    case 'complete_undo_unavailable': return null;
    case 'interrupt_not_allowed': return t('source.restore.blocked.interruptNotAllowed');
    default: return t('source.restore.blocked.other');
  }
}

/** A plan that went stale, or files that changed under it: the server refused before writing, and a fresh plan is what helps. */
export function planWentStale(error: unknown): boolean {
  return error instanceof ApiError && (error.code === 'preflight.expired' || error.code === 'workspace.restore_conflict' || error.code === 'conflict.stale_revision');
}

/** The result of a finished restore: the state it put back and the one it kept to undo it. */
export function restoreResult(result: unknown): { snapshot: string | null; undo: string | null } {
  if (typeof result !== 'object' || result === null) return { snapshot: null, undo: null };
  const record = result as Record<string, unknown>;
  return {
    snapshot: typeof record.snapshot_id === 'string' ? record.snapshot_id : null,
    undo: typeof record.undo_snapshot_id === 'string' ? record.undo_snapshot_id : null,
  };
}

/** The pull request page a push answered with, where there is one. */
export function pullRequestUrl(result: unknown): string | null {
  if (typeof result !== 'object' || result === null) return null;
  const value = (result as Record<string, unknown>).pull_request_url;
  return typeof value === 'string' && /^https?:\/\//.test(value) ? value : null;
}

/** One observed fact of a thread's work as a chip: shown only when it is known to be so; unknown is said, never drawn as clean. */
export type WorkChip = { kind: 'uncommitted' | 'unpushed' | 'failed'; stale: boolean } | { kind: 'unknown'; stale: boolean };

/** A reason for missing or older work facts, without exposing a protocol code; a plain directory is normal and has none. */
export function workReason(t: TFunction, reason: string | null): string | null {
  switch (reason) {
    case null: return null;
    // Nothing needs Git: a directory without a repository is not a fault to explain.
    case 'not_a_repository': return null;
    case 'source_offline': return t('source.work.reasons.offline');
    case 'git_missing': return t('source.work.reasons.gitMissing');
    case 'no_upstream': return t('source.work.reasons.noUpstream');
    case 'upstream_unknown': return t('source.work.reasons.upstreamUnknown');
    case 'files_changed': return t('source.work.reasons.changed');
    case 'coverage_unknown': return t('source.work.reasons.coverageUnknown');
    case 'not_observed': return t('source.work.reasons.notObserved');
    default: return t('source.work.reasons.unavailable');
  }
}

/** The chips of a thread's observed work: what is true, then one "unknown" for what could not be observed; a plain directory shows only a failed turn. */
export function workChips(attention: ThreadAttention | DirectoryWork | null | undefined): WorkChip[] {
  if (attention === null || attention === undefined) return [];
  if (attention.reason === 'not_a_repository') return 'failed' in attention && attention.failed ? [{ kind: 'failed', stale: false }] : [];
  const chips: WorkChip[] = [];
  if (attention.uncommitted === true) chips.push({ kind: 'uncommitted', stale: attention.stale });
  if (attention.unpushed === true) chips.push({ kind: 'unpushed', stale: attention.stale });
  if ('failed' in attention && attention.failed) chips.push({ kind: 'failed', stale: false });
  if ((attention.uncommitted === null || attention.unpushed === null) && attention.reason !== null) chips.push({ kind: 'unknown', stale: attention.stale });
  return chips;
}

/** Whether a thread needs somebody: only what is known to be true counts. */
export function needsWork(attention: ThreadAttention | null | undefined): boolean {
  if (attention?.reason === 'not_a_repository') return attention.failed;
  return attention?.uncommitted === true || attention?.unpushed === true || attention?.failed === true;
}

/** The most links a thread keeps: a short list people read at a glance, not a bookmark store. */
export const MOST_LINKS = 20;
/** The longest label of a link, in characters. */
export const LINK_LABEL_MOST = 80;

/** Why a link cannot be kept, or null when it can: an address of the web, without a name and password in it. */
export function linkFault(t: TFunction, link: ThreadLink): string | null {
  let parsed: URL;
  try {
    parsed = new URL(link.url.trim());
  } catch {
    return t('source.links.fault.address');
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return t('source.links.fault.scheme');
  if (parsed.username !== '' || parsed.password !== '') return t('source.links.fault.credentials');
  if (graphemeCount(link.label) > LINK_LABEL_MOST) return t('source.links.fault.label', { count: LINK_LABEL_MOST });
  return null;
}

/** One hunk of a unified difference: where it starts on each side, how long it is, and its text with its header. */
export interface Hunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  text: string;
}

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** The hunks of a unified difference, in order; a difference with none (a binary change, a cut one) has none. */
export function hunksOf(diff: string): Hunk[] {
  const hunks: Hunk[] = [];
  let current: { hunk: Hunk; lines: string[] } | null = null;
  for (const line of diff.split('\n')) {
    const header = HUNK_HEADER.exec(line);
    if (header !== null) {
      if (current !== null) hunks.push({ ...current.hunk, text: current.lines.join('\n') });
      current = {
        hunk: { oldStart: Number(header[1]), oldLines: header[2] === undefined ? 1 : Number(header[2]), newStart: Number(header[3]), newLines: header[4] === undefined ? 1 : Number(header[4]), text: '' },
        lines: [line],
      };
      continue;
    }
    if (current !== null && (line.startsWith(' ') || line.startsWith('+') || line.startsWith('-') || line.startsWith('\\'))) current.lines.push(line);
  }
  if (current !== null) hunks.push({ ...current.hunk, text: current.lines.join('\n') });
  return hunks;
}

/** The lines a hunk covers on one side, first and last; a side with no lines (a new or a deleted file) covers its start alone. */
export function hunkRange(hunk: Hunk, side: ReviewComment['side']): { from: number; to: number } {
  const start = side === 'old' ? hunk.oldStart : hunk.newStart;
  const count = side === 'old' ? hunk.oldLines : hunk.newLines;
  const from = Math.max(1, start);
  return { from, to: Math.max(from, start + count - 1) };
}

/** A hash of the reviewed difference as the reader saw it: the same text gives the same name on every device. */
export async function reviewedHash(text: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

/** The one message the comments on lines make, in the order they were written: each with its file, side and lines, the hunk it was written on and the words. */
export function reviewMessage(t: TFunction, comments: readonly ReviewComment[]): string {
  return comments
    .map((comment) => {
      const where = comment.from_line === comment.to_line
        ? t('source.review.message.line', { path: comment.path, line: comment.from_line, side: t(`source.review.side.${comment.side}`) })
        : t('source.review.message.lines', { path: comment.path, from: comment.from_line, to: comment.to_line, side: t(`source.review.side.${comment.side}`) });
      return `${where}\n\n\`\`\`diff\n${comment.hunk}\n\`\`\`\n\n${comment.text.trim()}`;
    })
    .join('\n\n---\n\n');
}

/** Whether the live difference no longer holds the hunk a comment was written on: the comment keeps it, and says so. */
export function hunkChanged(comment: ReviewComment, liveDiff: string | null): boolean {
  if (liveDiff === null) return true;
  return !hunksOf(liveDiff).some((hunk) => hunk.text === comment.hunk);
}
