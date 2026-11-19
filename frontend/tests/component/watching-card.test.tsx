/** An expected watch-only access decision offers a way to ask, without pretending that retrying the same request grants access. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { WatchingCard } from '@/features/thread/WatchingCard';
import type { MissingAccess, Thread, UserRef } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ reload: vi.fn() }));
vi.mock('@/realtime/live', () => ({ reloadThread: calls.reload }));
vi.mock('@/features/access/AccessRequest', () => ({
  AccessRequest: ({ onChanged }: { onChanged: () => void }) => <button onClick={onChanged}>Access was granted</button>,
}));

const owner: UserRef = { id: 'owner', username: 'dana', display_name: 'Dana', kind: 'human', avatar: { text: 'D', color: 1 } };
const missing: MissingAccess = {
  object: { id: 'thread-1', kind: 'thread', name: 'Repair login', owner },
  permissions: ['thread.interact'], options: null, override: false, closed: null, ask: [owner], ask_more: 0,
};

function draw(closed: MissingAccess['closed'] = null) {
  const thread = { id: 'thread-1', name: 'Repair login', owner, environment: null, input_access: { allowed: false, missing: [{ ...missing, closed }] } } as unknown as Thread;
  show(<WatchingCard thread={thread} />);
  return screen.getByRole('region', { name: 'You can watch this thread' });
}

beforeEach(() => { calls.reload.mockClear(); });
afterEach(async () => { cleanup(); await setLocale('en'); });

describe('a viewer of a thread', () => {
  it('explains the expected refusal and offers access and chat without a retry', () => {
    const card = within(draw());
    expect(card.getByText(/to talk to its agent/)).toBeVisible();
    expect(card.getByRole('button', { name: 'Request access' })).toBeVisible();
    expect(card.getByRole('button', { name: 'Ask in chat' })).toBeVisible();
    expect(card.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
    expect(calls.reload).not.toHaveBeenCalled();
  });

  it('still refreshes the thread after an access request changes its authority', async () => {
    const user = userEvent.setup();
    const card = within(draw());
    await user.click(card.getByRole('button', { name: 'Request access' }));
    await user.click(screen.getByRole('button', { name: 'Access was granted' }));
    expect(calls.reload).toHaveBeenCalledExactlyOnceWith('thread-1');
  });

  it.each(['en', 'zh-CN'] as const)('names managers and the model for a person, once, without routing strings in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const thread = { id: 'thread-1', name: 'Repair login', owner, environment: null, model: 'vendor/internal-model-route-2026', model_display: 'Reasoner', effort: 'high',
      input_access: { allowed: false, missing: [{ ...missing, object: { ...missing.object, kind: 'provider', name: 'Research provider' }, options: { models: ['vendor/internal-model-route-2026'], efforts: ['high'] } }] } } as unknown as Thread;
    show(<WatchingCard thread={thread} />);
    const card = within(screen.getByRole('region', { name: t('thread.watching.label') }));
    expect(card.getAllByText(t('access.request.title'), { exact: true })).toHaveLength(1);
    expect(card.getByText(t('thread.watching.askWho', { names: 'Dana' }))).toBeVisible();
    expect(card.getByText(/Reasoner/)).toBeVisible();
    expect(card.queryByText(/vendor\/internal-model-route/)).not.toBeInTheDocument();
    expect(card.queryByRole('textbox')).not.toBeInTheDocument();
    expect(card.queryByRole('group', { name: t('composer.row.label') })).not.toBeInTheDocument();
  });
});
