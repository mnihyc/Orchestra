/** The compact viewer line retains native recipients, model words and the existing request recovery. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { ViewerAccessLine } from '@/features/composer/ViewerAccessLine';
import type { MissingAccess, Thread, UserRef } from '@/generated/api-types';
import { defaultThreadRead } from '@/realtime/threadScope';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ reload: vi.fn() }));
vi.mock('@/realtime/live', async (loadOriginal) => ({ ...await loadOriginal<object>(), reloadThread: calls.reload }));
vi.mock('@/features/access/AccessRequest', () => ({
  AccessRequest: ({ onChanged }: { onChanged: () => void }) => <button onClick={onChanged}>Access was granted</button>,
}));

const owner: UserRef = { id: 'owner', username: 'dana', display_name: 'Dana', kind: 'human', avatar: { text: 'D', color: 1 } };
const missing: MissingAccess = {
  object: { id: 'thread-1', kind: 'thread', name: 'Repair login', owner },
  permissions: ['thread.interact'], options: null, override: false, closed: null, ask: [owner], ask_more: 0,
};

let thread: Thread;
function draw(closed: MissingAccess['closed'] = null) {
  show(<ViewerAccessLine thread={{ ...thread, input_access: { allowed: false, missing: [{ ...missing, closed }] } }} />);
  return screen.getByRole('group', { name: i18n.t('thread.watching.label') });
}

beforeEach(() => {
  calls.reload.mockClear();
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The viewer line needs a complete native thread');
  thread = { ...world.threads.thread(person, row), id: missing.object.id, name: 'Repair login', owner, environment: null };
});
afterEach(async () => { cleanup(); await setLocale('en'); });

describe('a viewer of a thread', () => {
  it('names the missing Interact access and its giver with one request action and no retry', () => {
    const card = within(draw());
    expect(card.getByText(/Interact on Repair login/)).toBeVisible();
    expect(card.getByRole('button', { name: 'Request access' })).toBeVisible();
    expect(card.getAllByRole('button')).toHaveLength(1);
    expect(card.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
    expect(calls.reload).not.toHaveBeenCalled();
  });

  it('still refreshes the thread after an access request changes its authority', async () => {
    const user = userEvent.setup();
    const card = within(draw());
    await user.click(card.getByRole('button', { name: 'Request access' }));
    await user.click(screen.getByRole('button', { name: 'Access was granted' }));
    expect(calls.reload).toHaveBeenCalledExactlyOnceWith(defaultThreadRead('thread-1'));
  });

  it.each(['en', 'zh-CN'] as const)('names managers and the model for a person, once, without routing strings in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const viewer: Thread = { ...thread, model: 'vendor/internal-model-route-2026', model_display: 'Reasoner', effort: 'high',
      input_access: { allowed: false, missing: [{ ...missing, object: { ...missing.object, kind: 'provider', name: 'Research provider' }, options: { models: ['vendor/internal-model-route-2026'], efforts: ['high'] } }] } };
    show(<ViewerAccessLine thread={viewer} />);
    const card = within(screen.getByRole('group', { name: t('thread.watching.label') }));
    expect(card.getAllByText(t('access.request.title'), { exact: true })).toHaveLength(1);
    expect(card.getByText(t('thread.watching.askWho', { names: 'Dana' }))).toBeVisible();
    expect(card.getByText(/Reasoner/)).toBeVisible();
    expect(card.queryByText(/vendor\/internal-model-route/)).not.toBeInTheDocument();
    expect(card.queryByRole('textbox')).not.toBeInTheDocument();
    expect(card.queryByRole('group', { name: t('composer.row.label') })).not.toBeInTheDocument();
  });

  it('keeps a closed request unavailable without opening an empty request form', () => {
    const card = within(draw('subscription_not_shared'));
    expect(card.getByText(i18n.t('thread.watching.nobody'))).toBeVisible();
    expect(card.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('retains a declared additional giver without inventing their identity or a distinct-person count', () => {
    show(<ViewerAccessLine thread={{ ...thread, input_access: { allowed: false, missing: [{ ...missing, ask: [], ask_more: 2 }] } }} />);
    const line = within(screen.getByRole('group', { name: i18n.t('thread.watching.label') }));
    expect(line.getByText(i18n.t('thread.watching.askOthers'))).toBeVisible();
    expect(line.getByRole('button', { name: i18n.t('access.request.title') })).toBeEnabled();
    expect(line.queryByText(owner.display_name)).not.toBeInTheDocument();
  });
});
