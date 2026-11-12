// @vitest-environment node
/** Composer sends and answer-row retries share uncertainty, reader scope and acknowledgement semantics. */
import { expect, it, vi } from 'vitest';
import { askDraftKey, askSendIntent, completeAskSendIntent } from '@/features/composer/askDrafts';

it('replays the original body/callback after a lost acknowledgement and creates a new intent only after success', async () => {
  const key = `${askDraftKey('replay-root', 'conversation')}:retry:answer`, bodies: { id: string; context: string }[] = [];
  const accepted = new Set<string>(); let loseAcknowledgement = true;
  const send = vi.fn((id: string) => {
    bodies.push({ id, context: 'original-page' }); accepted.add(id);
    if (loseAcknowledgement) { loseAcknowledgement = false; return Promise.reject(new Error('The connection ended after admission')); }
    return Promise.resolve();
  });
  const first = askSendIntent(key, 'question-and-model', send);
  await expect(first.send()).rejects.toThrow('after admission');
  const movedPage = vi.fn((id: string) => { bodies.push({ id, context: 'another-page' }); return Promise.resolve(); });
  const retry = askSendIntent(key, 'question-and-model', movedPage);
  await retry.send(); completeAskSendIntent(key, retry);
  expect(bodies).toEqual([{ id: first.id, context: 'original-page' }, { id: first.id, context: 'original-page' }]);
  expect(accepted.size).toBe(1); expect(movedPage).not.toHaveBeenCalled();
  const next = askSendIntent(key, 'question-and-model', movedPage);
  expect(next.id).not.toBe(first.id); await next.send(); completeAskSendIntent(key, next);
  expect(movedPage).toHaveBeenCalledWith(next.id);
});

it('separates roots, conversations and answer rows, and a personal edit cannot be retired by an older acknowledgement', () => {
  const root = askDraftKey('scope-root', 'conversation'), otherRoot = askDraftKey('other-root', 'conversation'), otherConversation = askDraftKey('scope-root', 'other-conversation');
  const send = vi.fn(() => Promise.resolve()), fingerprint = 'same-content-and-model';
  const first = askSendIntent(`${root}:retry:first`, fingerprint, send);
  const differentRow = askSendIntent(`${root}:retry:second`, fingerprint, send);
  const differentRoot = askSendIntent(`${otherRoot}:retry:first`, fingerprint, send);
  const differentConversation = askSendIntent(`${otherConversation}:retry:first`, fingerprint, send);
  expect(new Set([first.id, differentRow.id, differentRoot.id, differentConversation.id]).size).toBe(4);
  const edited = askSendIntent(`${root}:retry:first`, 'same-question-with-another-model', send);
  expect(edited.id).not.toBe(first.id); completeAskSendIntent(`${root}:retry:first`, first);
  expect(askSendIntent(`${root}:retry:first`, 'same-question-with-another-model', send).id).toBe(edited.id);
  for (const [key, attempt] of [[`${root}:retry:first`, edited], [`${root}:retry:second`, differentRow], [`${otherRoot}:retry:first`, differentRoot], [`${otherConversation}:retry:first`, differentConversation]] as const) completeAskSendIntent(key, attempt);
});
