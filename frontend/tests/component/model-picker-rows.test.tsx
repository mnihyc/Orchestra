/** The composer's real model/effort menu uses full-width, borderless picker rows and explicit activation. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { effortDisplayName } from '@/features/models/names';
import type { ModelOption, ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const server = vi.hoisted(() => ({ settings: new Map<string, ResolvedSetting>(), parameters: new Map<string, ThreadParameters>(), write: vi.fn<typeof ThreadsApi.setThreadSetting>() }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve([...server.settings.values()]), staleTime: Infinity }) }));
vi.mock('@/api/threads', async (load) => ({
  ...await load<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['thread-parameters', id], queryFn: () => {
    const parameters = server.parameters.get(id);
    if (parameters === undefined) throw new Error('The picker needs the native parameters of its thread');
    return Promise.resolve(parameters);
  }, staleTime: Infinity }),
  setThreadSetting: (...args: Parameters<typeof ThreadsApi.setThreadSetting>) => server.write(...args),
}));
vi.mock('@/features/usage/PerformancePlaces', () => ({ useChoicePerformance: () => () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const options: ModelOption[] = [
  { ...MODEL_ESTIMATE, model_name: 'alpha-routing', display_name: 'Alpha', allowed: true, reason: null, parameters: { efforts: ['low', 'high'], default_effort: 'high' } },
  { ...MODEL_ESTIMATE, model_name: 'bravo-routing', display_name: 'Bravo', allowed: true, reason: null, parameters: {} },
];
const setting = (key: string, value: unknown, revision: number): ResolvedSetting => ({ key, value, own: { value, revision, locked: false }, source: { scope: 'thread', node: null }, locked_by: null, writable: true, parts: [] });
let native: Thread;
let previousTheme: string | null;
const styles: HTMLStyleElement[] = [];
// CSS imports are empty in the component runner. Read the shipped selectors; the browser companion measures their geometry.
const choiceStyles = readFileSync(resolve(import.meta.dirname, '../../src/styles/choices.css'), 'utf8');
function stylesheet(dark: boolean) {
  const colours: Record<string, string> = { surface: dark ? '#111111' : '#ffffff', 'control-border': dark ? '#bbbbbb' : '#444444',
    'control-selected': dark ? '#eeeeee' : '#111111', 'control-on-selected': dark ? '#111111' : '#ffffff', 'bg-muted': dark ? '#333333' : '#dddddd' };
  const style = document.createElement('style');
  style.textContent = choiceStyles.replace(/var\(--([\w-]+)\)/g, (original: string, token: string) => colours[token] ?? original);
  document.head.append(style); styles.push(style);
  expect(style.sheet).not.toBeNull(); expect(style.sheet?.cssRules.length).toBeGreaterThan(0);
}

beforeEach(() => {
  previousTheme = document.documentElement.getAttribute('data-theme');
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The model picker needs its native thread');
  native = { ...world.threads.thread(person, row), model: 'alpha-routing', model_display: 'Alpha', effort: 'high' };
  server.parameters.clear(); server.settings.clear(); server.write.mockReset();
  server.parameters.set(native.id, { model: { value: 'alpha-routing', options, writable: true, source: { scope: 'thread', node: null } },
    effort: { value: 'high', writable: true, supported: true, source: { scope: 'thread', node: null } }, parameters: [], approval_mode: 'on_request', approval_reviewer: null });
  for (const one of [setting('agent.model', 'alpha-routing', 3), setting('agent.effort', 'high', 7), setting('agent.params', {}, 1)]) server.settings.set(one.key, one);
  server.write.mockImplementation((_id, key, body) => Promise.resolve(setting(key, body.value, (server.settings.get(key)?.own?.revision ?? 0) + 1)));
});
afterEach(async () => {
  cleanup(); threadWriter.evict(native.id);
  for (const style of styles.splice(0)) style.remove();
  if (previousTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', previousTheme);
  await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`gives model and effort options full-width picker rows, transparent borders and check marks in ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.setAttribute('data-theme', theme); stylesheet(theme === 'dark');
    show(<ModelEffort thread={native} variant="composer" open />);
    const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.title') });
    const menu = await within(picker).findByRole('menu', { name: i18n.t('composer.model.list') });
    const alpha = within(menu).getByRole('menuitemradio', { name: 'Alpha', checked: true });
    const bravo = within(menu).getByRole('menuitemradio', { name: 'Bravo', checked: false });
    const efforts = within(menu).getByRole('group', { name: i18n.t('composer.model.efforts', { model: 'Alpha' }) });
    const low = within(efforts).getByRole('menuitemradio', { name: effortDisplayName('low', i18n.t), checked: false });
    const high = within(efforts).getByRole('menuitemradio', { name: effortDisplayName('high', i18n.t), checked: true });
    expect(menu).toHaveAttribute('aria-orientation', 'vertical'); expect(menu).toHaveClass('w-full'); expect(efforts).toHaveClass('w-full');
    expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(4);
    for (const option of [alpha, low, high, bravo]) {
      const choice = option.closest('[data-choice-layout]');
      expect(choice).toHaveAttribute('data-choice-layout', 'picker'); expect(choice).toHaveClass('w-full');
      expect(option).toHaveClass('w-full'); expect(option).not.toHaveAttribute('aria-pressed');
      expect(getComputedStyle(option).width).not.toBe('max-content');
      expect(getComputedStyle(option).borderTopColor).toBe('rgba(0, 0, 0, 0)');
      expect(getComputedStyle(option).backgroundColor).toBe('rgba(0, 0, 0, 0)');
      expect(option.querySelectorAll('svg.lucide-check')).toHaveLength(option.getAttribute('aria-checked') === 'true' ? 1 : 0);
    }
    expect(within(menu).getAllByText('Alpha', { exact: true })).toHaveLength(1);
    expect(within(menu).getAllByText('Bravo', { exact: true })).toHaveLength(1);
    expect(alpha.closest('li')).not.toHaveClass('bg-accent-subtle');
    expect(server.write).not.toHaveBeenCalled();
  });

  it(`moves focus without changing a choice and activates an effort with its setting revisions in ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
    const user = userEvent.setup(); show(<ModelEffort thread={native} variant="composer" open />);
    const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.title') });
    const menu = await within(picker).findByRole('menu', { name: i18n.t('composer.model.list') });
    const alpha = within(menu).getByRole('menuitemradio', { name: 'Alpha' }), bravo = within(menu).getByRole('menuitemradio', { name: 'Bravo' });
    const low = within(menu).getByRole('menuitemradio', { name: effortDisplayName('low', i18n.t) });
    await waitFor(() => expect(alpha).toBeEnabled());
    expect(within(menu).getAllByRole('menuitemradio').filter((option) => option.tabIndex === 0)).toEqual([alpha]);
    await user.click(within(picker).getByRole('textbox', { name: i18n.t('composer.model.find') }));
    await user.keyboard('{ArrowDown}'); expect(alpha).toHaveFocus();
    await user.keyboard('{ArrowDown}'); expect(low).toHaveFocus();
    expect(within(menu).getAllByRole('menuitemradio').filter((option) => option.tabIndex === 0)).toEqual([low]);
    await user.keyboard('{End}'); expect(bravo).toHaveFocus();
    await user.keyboard('{ArrowDown}'); expect(alpha).toHaveFocus();
    await user.keyboard('{ArrowUp}'); expect(bravo).toHaveFocus();
    await user.keyboard('{Home}{ArrowDown}'); expect(low).toHaveFocus();
    expect(server.write).not.toHaveBeenCalled();
    await user.keyboard(' ');
    await waitFor(() => expect(server.write).toHaveBeenCalledTimes(2));
    expect(server.write).toHaveBeenNthCalledWith(1, native.id, 'agent.model', { value: 'alpha-routing', expected_revision: 3 });
    expect(server.write).toHaveBeenNthCalledWith(2, native.id, 'agent.effort', { value: 'low', expected_revision: 7 });
  });
}

it('finds a model by either name and moves focus to a filtered row without selecting it', async () => {
  const user = userEvent.setup(); show(<ModelEffort thread={native} variant="composer" open />);
  const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.title') });
  const search = await within(picker).findByRole('textbox', { name: i18n.t('composer.model.find') });
  await within(picker).findByRole('menuitemradio', { name: 'Alpha' });
  await user.type(search, 'bravo-routing');
  const bravo = within(picker).getByRole('menuitemradio', { name: 'Bravo', checked: false });
  expect(within(picker).queryByRole('menuitemradio', { name: 'Alpha' })).not.toBeInTheDocument();
  await user.keyboard('{ArrowDown}'); expect(bravo).toHaveFocus();
  await user.click(search); await user.clear(search); await user.type(search, 'Alpha');
  await user.keyboard('{ArrowUp}'); expect(within(picker).getByRole('menuitemradio', { name: effortDisplayName('high', i18n.t) })).toHaveFocus();
  expect(server.write).not.toHaveBeenCalled();
});

it('keeps every model and effort read-only for a driver who cannot manage the thread', async () => {
  const user = userEvent.setup();
  show(<ModelEffort thread={{ ...native, access: { ...native.access, permissions: native.access.permissions.filter((permission) => permission !== 'thread.manage') } }} variant="composer" open />);
  const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.title') });
  const menu = await within(picker).findByRole('menu', { name: i18n.t('composer.model.list') });
  expect(within(picker).getByText(i18n.t('composer.model.readOnly', { who: native.owner.display_name }))).toBeVisible();
  for (const option of within(menu).getAllByRole('menuitemradio')) { expect(option).toBeDisabled(); expect(option).toHaveAttribute('tabindex', '-1'); await user.click(option); }
  expect(server.write).not.toHaveBeenCalled();
});
