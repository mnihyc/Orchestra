/** The rules of the source screens: a plan is confirmed only when it writes, keeps its undo and nothing stops it; unknown work is never clean; a link is an address of the web without a password; comments on lines keep their hunk and make one message. */
import { describe, expect, it } from 'vitest';

import { ApiError } from '@/api/client';
import { confirmable, hunkChanged, hunkRange, hunksOf, linkFault, needsWork, planWentStale, pullRequestUrl, restorable, restoreBlocker, restoreResult, reviewMessage, sourceFailure, unrecordedReason, workChips, workReason } from '@/features/source/words';
import type { Problem, RestorePlan, ReviewComment, Snapshot } from '@/generated/api-types';
import { i18n, loadSearchCatalogues, setLocale } from '@/i18n';

const t = i18n.t.bind(i18n);

it.each(['en', 'zh-CN'] as const)('uses catalogue sentences for every recording reason and unknown failure in %s', async (locale) => {
  await setLocale(locale);
  try {
    for (const [reason, key] of [
      ['live_tree', 'source.restore.why.liveTree'], ['not_ready', 'source.restore.why.notReady'],
      ['store_unavailable', 'source.restore.why.storeUnavailable'], ['too_large', 'source.restore.why.tooLarge'],
      ['off', 'source.restore.why.recordingOff'], ['offline', 'source.restore.why.offline'],
      ['future_recording_failure', 'source.restore.why.other'],
    ] as const) {
      const words = unrecordedReason(i18n.t, snapshot({ state: 'unavailable', unavailable_reason: reason }));
      expect(words).toBe(i18n.t(key));
      if (reason.includes('_')) expect(words).not.toContain(reason);
      if (locale === 'zh-CN') expect(words).toMatch(/\p{Script=Han}/u);
    }
    expect(workReason(i18n.t, 'future_work_failure')).toBe(i18n.t('source.work.reasons.unavailable'));
    expect(sourceFailure(i18n.t, 'future_source_failure')).toBe(i18n.t('source.prepare.failureReason'));
  } finally { await setLocale('en'); }
});

const DIFF = ['diff --git a/src/login.ts b/src/login.ts', '--- a/src/login.ts', '+++ b/src/login.ts', '@@ -10,3 +10,4 @@ export function login()', ' const user = find();', '-if (!user) return;', '+if (!user) {', '+  throw new Error("no user");', ' }', '@@ -40 +41 @@', '-old', '+new'].join('\n');

function snapshot(change: Partial<Snapshot> = {}): Snapshot {
  return {
    completed_at: '2026-10-14T10:00:00.000Z', created_at: '2026-10-14T10:00:00.000Z', created_by: null, directory: 'web', env_generation: 1,
    environment: { id: 'e', name: 'Main' } as Snapshot['environment'], expires_at: null, file_count: 12, id: 's', kind: 'turn', label: '', missing_count: 0,
    repository_head: null, selection: { include: [], exclude: [] }, state: 'complete', storage: 'environment', thread_id: 't', total_bytes: 100, unavailable_reason: null, ...change,
  };
}

function plan(change: Partial<RestorePlan> = {}): RestorePlan {
  return {
    blockers: [], directory: 'web', environment: { id: 'e', name: 'Main' } as RestorePlan['environment'], expires_at: '2026-10-14T10:15:00.000Z', generation: 1, ignore_fingerprint: 'i', ignored: [], namespace_fingerprint: 'n',
    options: { active_turn: 'wait', keep_previous: false, stop_processes: false, verify_timeout_secs: 60, wait_minutes: 10 },
    paths: [{ action: 'change', expected: 'h', later_change: false, later_writers: [], path: 'src/login.ts' }], preflight_id: 'p', snapshot: snapshot(), undo_available: true, writers: [], ...change,
  };
}

describe('a whole directory is put back only from a complete record, through a plan that keeps its undo', () => {
  it('restores only a complete record with nothing missing', () => {
    expect(restorable(snapshot())).toBe(true);
    expect(restorable(snapshot({ state: 'partial' }))).toBe(false);
    expect(restorable(snapshot({ missing_count: 1 }))).toBe(false);
  });
  it('confirms a plan only when it writes something, keeps a complete undo and nothing stops it', () => {
    expect(confirmable(plan())).toBe(true);
    expect(confirmable(plan({ paths: [] }))).toBe(false);
    expect(confirmable(plan({ undo_available: false }))).toBe(false);
    expect(confirmable(plan({ blockers: ['The environment is stopped.'] }))).toBe(false);
  });
  it('says why nothing complete was recorded, in words and never as a code', () => {
    expect(unrecordedReason(t, null)).toBe('Files of this turn were not recorded.');
    expect(unrecordedReason(t, snapshot({ state: 'unavailable', unavailable_reason: 'live_tree' }))).toBe('Files of this turn were not recorded: the workspace is live.');
    expect(unrecordedReason(t, snapshot({ state: 'expired' }))).toBe('The files recorded before this turn are no longer kept.');
    expect(unrecordedReason(t, snapshot({ missing_count: 2 }))).toBe('2 recorded files are missing.');
    expect(unrecordedReason(t, snapshot({ state: 'unavailable', unavailable_reason: 'too_large' }))).toBe('The files were too large to record as a whole.');
  });
  it('asks for a fresh plan when the server refused a stale one before writing', () => {
    const problem = (code: Problem['code']) => new ApiError({ type: '', code, title: '', status: 409, detail: '', request_id: '' }, null);
    expect(planWentStale(problem('workspace.restore_conflict'))).toBe(true);
    expect(planWentStale(problem('preflight.expired'))).toBe(true);
    expect(planWentStale(problem('authz.denied'))).toBe(false);
  });
  it('reads the undo state and the pull request page from a job result, and nothing that is not there', () => {
    expect(restoreResult({ snapshot_id: 'a', undo_snapshot_id: 'b' })).toEqual({ snapshot: 'a', undo: 'b' });
    expect(restoreResult(null)).toEqual({ snapshot: null, undo: null });
    expect(pullRequestUrl({ pull_request_url: 'https://github.com/acme/web/compare/main...fix?expand=1' })).toContain('compare');
    expect(pullRequestUrl({ pull_request_url: 'javascript:alert(1)' })).toBeNull();
    expect(pullRequestUrl({ status: {} })).toBeNull();
  });
});

describe('observed work is a fact with its time, and unknown is never clean', () => {
  it('has no Git work chip or reason for a non-repository and still tells of a failed turn', () => {
    const directory = { uncommitted: null, unpushed: null, failed: false, as_of: null, stale: true, reason: 'not_a_repository' };
    expect(workChips(directory)).toEqual([]);
    expect(workReason(t, 'not_a_repository')).toBeNull();
    expect(needsWork(directory)).toBe(false);
    expect(workChips({ ...directory, failed: true })).toEqual([{ kind: 'failed', stale: false }]);
    expect(needsWork({ ...directory, failed: true })).toBe(true);
  });
  it('explains missing Git, an offline source and future reasons without protocol values in either language', async () => {
    expect(workReason(t, 'git_missing')).toBe('Git is not installed in this environment.');
    expect(workReason(t, 'source_offline')).toBe('The environment is offline; these are its last observed Git facts.');
    expect(workReason(t, 'new_machine_reason')).toBe('Git status could not be read.');
    await loadSearchCatalogues();
    const chinese = i18n.getFixedT('zh-CN');
    expect(workReason(chinese, 'source_offline')).toBe('环境已离线；这里保留的是上次观察到的 Git 状态。');
    expect(workReason(chinese, 'upstream_unknown')).toBe('无法读取与远程分支的比较结果。');
    expect(workReason(chinese, 'new_machine_reason')).toBe('无法读取 Git 状态。');
  });
  it('shows only what is known to be true, and one unknown with its reason', () => {
    expect(workChips({ uncommitted: true, unpushed: false, failed: false, as_of: null, stale: false, reason: null }).map((chip) => chip.kind)).toEqual(['uncommitted']);
    expect(workChips({ uncommitted: null, unpushed: true, failed: true, as_of: null, stale: true, reason: 'The environment was offline.' }).map((chip) => chip.kind)).toEqual(['unpushed', 'failed', 'unknown']);
    expect(workChips({ uncommitted: false, unpushed: false, failed: false, as_of: null, stale: false, reason: null })).toEqual([]);
    expect(needsWork({ uncommitted: null, unpushed: null, failed: false, as_of: null, stale: false, reason: 'not observed' })).toBe(false);
  });
});

describe('a thread keeps links of the web, never with a password in them', () => {
  it('refuses another scheme, credentials and a long label', () => {
    expect(linkFault(t, { url: 'https://github.com/acme/web/pull/7', label: 'PR 7' })).toBeNull();
    expect(linkFault(t, { url: 'ftp://example.org', label: '' })).not.toBeNull();
    expect(linkFault(t, { url: 'https://user:secret@example.org', label: '' })).not.toBeNull();
    expect(linkFault(t, { url: 'not an address', label: '' })).not.toBeNull();
    expect(linkFault(t, { url: 'https://example.org', label: 'x'.repeat(81) })).not.toBeNull();
  });
});

describe('comments on lines keep the hunk they were written on and make one message', () => {
  it('reads the hunks of a difference with the lines each covers on both sides', () => {
    const hunks = hunksOf(DIFF);
    expect(hunks).toHaveLength(2);
    expect(hunkRange(hunks[0]!, 'old')).toEqual({ from: 10, to: 12 });
    expect(hunkRange(hunks[0]!, 'new')).toEqual({ from: 10, to: 13 });
    expect(hunkRange(hunks[1]!, 'new')).toEqual({ from: 41, to: 41 });
    expect(hunksOf('Binary files differ')).toEqual([]);
  });
  it('says a comment is stale when the live difference no longer holds its hunk', () => {
    const hunk = hunksOf(DIFF)[0]!.text;
    const comment: ReviewComment = { id: 'c', path: 'src/login.ts', side: 'new', from_line: 11, to_line: 12, diff_hash: 'h', item_id: null, hunk, text: 'Throw a typed error.' };
    expect(hunkChanged(comment, DIFF)).toBe(false);
    expect(hunkChanged(comment, DIFF.replace('no user', 'nobody'))).toBe(true);
    expect(hunkChanged(comment, null)).toBe(true);
  });
  it('makes one message with the file, side, lines, hunk and words of each comment, in order', () => {
    const [first, second] = hunksOf(DIFF);
    const message = reviewMessage(t, [
      { id: 'a', path: 'src/login.ts', side: 'new', from_line: 11, to_line: 12, diff_hash: 'h', item_id: null, hunk: first!.text, text: ' Throw a typed error. ' },
      { id: 'b', path: 'src/login.ts', side: 'old', from_line: 40, to_line: 40, diff_hash: 'h', item_id: null, hunk: second!.text, text: 'Why was this removed?' },
    ]);
    expect(message.indexOf('lines 11 to 12 of the new version')).toBeLessThan(message.indexOf('line 40 of the old version'));
    expect(message).toContain('```diff\n@@ -10,3 +10,4 @@');
    expect(message).toContain('Throw a typed error.\n\n---');
  });
});

it.each(['en', 'zh-CN'] as const)('words why a restore plan is blocked, never as its code, and says a missing undo once (%s)', async (locale) => {
  await setLocale(locale);
  try {
    expect(restoreBlocker(i18n.t, 'interrupt_not_allowed')).toBe(i18n.t('source.restore.blocked.interruptNotAllowed'));
    // The missing undo has a line of its own in the plan.
    expect(restoreBlocker(i18n.t, 'complete_undo_unavailable')).toBeNull();
    const future = restoreBlocker(i18n.t, 'future_blocker');
    expect(future).toBe(i18n.t('source.restore.blocked.other'));
    expect(future).not.toContain('future_blocker');
  } finally { await setLocale('en'); }
});
