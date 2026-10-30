/** A model picker refuses an unseen competing switch, keeps the current choice visible and sends its new revision only on the next click. */
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterAll, beforeAll, beforeEach, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { ModelEffort } from '@/features/composer/ModelEffort';
import type { NodeSettingSetBody, ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';

import { show } from './support';
import { currentLocale, i18n, setLocale } from '@/i18n';

const server = vi.hoisted(() => ({ rows: [] as ResolvedSetting[], parameters: {} as ThreadParameters, write: vi.fn<(id: string, key: string, body: NodeSettingSetBody) => Promise<ResolvedSetting>>() }));
vi.mock('@/api/settings', async (importOriginal) => ({ ...await importOriginal<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve(server.rows), staleTime: Infinity }) }));
vi.mock('@/api/threads', async (importOriginal) => ({
  ...await importOriginal<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['thread-parameters', id], queryFn: () => Promise.resolve(server.parameters), staleTime: Infinity }),
  setThreadSetting: (id: string, key: string, body: NodeSettingSetBody) => server.write(id, key, body),
}));
vi.mock('@/realtime/live', () => ({ refreshThread: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const row = (key: string, value: unknown, revision: number): ResolvedSetting => ({ key, value, own: revision === 0 ? null : { value, revision, locked: false }, source: { scope: 'thread', node: null }, locked_by: null, writable: true, parts: [] });
const options = ['alpha', 'bravo'].map((model_name) => ({ ...MODEL_ESTIMATE, model_name, display_name: model_name === 'alpha' ? 'Alpha' : 'Bravo', allowed: true, reason: null, parameters: {} }));
const parameters = (value: string): ThreadParameters => ({ model: { value, options, writable: true, source: { scope: 'thread', node: null } }, effort: { value: null, writable: true, supported: false, source: { scope: 'default', node: null } }, parameters: [], approval_mode: 'ask', approval_reviewer: null });
const thread = { id: 'thread-1', model: 'alpha', model_display: 'Alpha', effort: null, binding: { provider: { id: 'provider-1' } }, owner: { display_name: 'Ada' }, access: { permissions: ['thread.manage'] } } as Thread;

// jsdom has no scrolling layout; keep the browser method available when the selected row changes.
const scrollIntoView = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
beforeAll(() => {
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, writable: true, value: vi.fn() });
});
afterAll(() => {
  if (scrollIntoView === undefined) Reflect.deleteProperty(Element.prototype, 'scrollIntoView');
  else Object.defineProperty(Element.prototype, 'scrollIntoView', scrollIntoView);
});

beforeEach(() => {
  server.write.mockReset();
  server.parameters = parameters('alpha');
  server.rows = [row('agent.model', 'alpha', 1), row('agent.effort', null, 0), row('agent.params', {}, 0)];
});

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('names each model once with one help entrance and a short search (%s, %s)', async (locale, theme) => {
  const previousLocale = currentLocale();
  const previousTheme = document.documentElement.getAttribute('data-theme');
  await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
  const view = show(<ModelEffort thread={thread} variant="composer" open />);
  try {
    const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.title') });
    const list = await within(picker).findByRole('menu', { name: i18n.t('composer.model.list') });
    expect(within(list).getAllByText('Alpha', { exact: true })).toHaveLength(1);
    expect(within(list).getAllByText('Bravo', { exact: true })).toHaveLength(1);
    expect(within(picker).queryByText(/Use this model in this thread\.|在(?:这|此)(?:个)?会话中使用(?:此|该|这个)?模型[。.]?/u)).not.toBeInTheDocument();
    expect(within(picker).getByRole('textbox')).toHaveAttribute('placeholder', locale === 'en' ? 'Find a model' : '查找模型');
    await waitFor(() => expect(picker.querySelectorAll('button[data-help-guide]')).toHaveLength(1));
  } finally {
    view.unmount(); await setLocale(previousLocale);
    if (previousTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', previousTheme);
  }
});

it.each([false, true])('keeps the current model selected, names only its confirmed changer, and guards the next click (a later write overtakes the refusal: %s)', async (overtaken) => {
  const user = userEvent.setup();
  show(<ModelEffort thread={thread} variant="composer" open />);
  const picker = await screen.findByRole('dialog', { name: 'Model and effort' });
  const alpha = await within(picker).findByRole('menuitemradio', { name: /^Alpha/, checked: true });
  await waitFor(() => expect(alpha).toBeEnabled());
  // The other page's write is committed but its feed has not reached this page yet.
  server.parameters = parameters('bravo');
  server.rows = [row('agent.model', 'bravo', 2), row('agent.effort', null, 0), row('agent.params', {}, 0)];
  server.write.mockImplementation((_id, key, body) => {
    const current = server.rows.find((one) => one.key === key);
    if (body.expected_revision !== (current?.own?.revision ?? 0)) {
      const error = new ApiError({ type: 'orch:error:conflict.stale_revision', status: 409, title: 'The setting changed', code: 'conflict.stale_revision', detail: 'The setting changed.', request_id: 'r1', current, changed_by: { id: 'ada', display_name: 'Ada' } }, null);
      if (overtaken) server.rows = server.rows.map((one) => one.key === key ? row(key, 'bravo', 3) : one);
      return Promise.reject(error);
    }
    const next = row(key, body.value, (current?.own?.revision ?? 0) + 1);
    server.rows = server.rows.map((one) => one.key === key ? next : one);
    return Promise.resolve(next);
  });
  await user.click(alpha);
  const notice = await within(picker).findByRole('status');
  if (overtaken) { expect(notice).toHaveTextContent('The current model is Bravo.'); expect(notice).not.toHaveTextContent('Ada'); }
  else expect(notice).toHaveTextContent('Ada switched to Bravo just now.');
  expect(screen.getByRole('dialog', { name: 'Model and effort' })).toBeVisible();
  expect(within(picker).getByRole('menuitemradio', { name: /^Bravo/, checked: true })).toBeVisible();
  expect(server.write).toHaveBeenCalledTimes(1);
  expect(server.write).toHaveBeenLastCalledWith(thread.id, 'agent.model', { value: 'alpha', expected_revision: 1 });
  await waitFor(() => expect(within(picker).getByRole('menuitemradio', { name: /^Alpha/ })).toBeEnabled());
  await user.click(within(picker).getByRole('menuitemradio', { name: /^Alpha/ }));
  await waitFor(() => expect(server.write).toHaveBeenCalledTimes(2));
  expect(server.write).toHaveBeenLastCalledWith(thread.id, 'agent.model', { value: 'alpha', expected_revision: overtaken ? 3 : 2 });
});
