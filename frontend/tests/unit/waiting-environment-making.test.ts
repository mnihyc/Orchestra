/** A thread that waits for an environment that is being made says so, instead of saying that it wakes one. */
import { expect, it } from 'vitest';
import { waitingWords } from '@/features/timeline/waiting';
import { i18n } from '@/i18n';
import { failedStart } from '../component/failed-start-fixtures';

function waiting(status: 'provisioning' | 'stopped') {
  const native = failedStart().thread;
  if (native.environment === null) throw new Error('the fixture thread has an environment');
  return { ...native, state: 'starting' as const, blocked: null, lock: null, starting_step: 'environment' as const,
    environment: { ...native.environment, status } };
}

it('says that the environment is being made while it is made', () => {
  expect(waitingWords(waiting('provisioning'), i18n.t.bind(i18n))).toBe(i18n.t('thread.starting.environmentMaking'));
});

it('says that it wakes an environment that already exists', () => {
  expect(waitingWords(waiting('stopped'), i18n.t.bind(i18n))).toBe(i18n.t('thread.starting.environment'));
});
