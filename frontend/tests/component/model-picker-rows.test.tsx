/** The composer's model, effort, speed and access program are separate compact controls; the model's list holds models alone and the effort's list the efforts of the chosen model. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ModelsApi from '@/api/models';
import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { effortDisplayName } from '@/features/models/names';
import type { ModelOption, ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const server = vi.hoisted(() => ({ settings: new Map<string, ResolvedSetting>(), parameters: new Map<string, ThreadParameters>(), write: vi.fn<typeof ThreadsApi.setThreadSetting>(), choices: vi.fn<typeof ModelsApi.changeThreadChoices>() }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve([...server.settings.values()]), staleTime: Infinity }) }));
vi.mock('@/api/models', async (load) => ({ ...await load<typeof ModelsApi>(), changeThreadChoices: (...args: Parameters<typeof ModelsApi.changeThreadChoices>) => server.choices(...args) }));
vi.mock('@/api/threads', async (load) => ({
  ...await load<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['thread-parameters', id], queryFn: () => {
    const parameters = server.parameters.get(id);
    if (parameters === undefined) throw new Error('The picker needs the native parameters of its thread');
    return Promise.resolve(parameters);
  }, staleTime: Infinity }),
  setThreadSetting: (...args: Parameters<typeof ThreadsApi.setThreadSetting>) => server.write(...args),
}));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

const SIX = ['minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
const options: ModelOption[] = [
  { ...MODEL_ESTIMATE, model_name: 'alpha-routing', display_name: 'Alpha', allowed: true, reason: null, parameters: { efforts: ['low', 'high'], default_effort: 'high', context_window: 400_000 } },
  { ...MODEL_ESTIMATE, model_name: 'bravo-routing', display_name: 'Bravo', allowed: true, reason: null, parameters: {} },
  { ...MODEL_ESTIMATE, model_name: 'charlie-routing', display_name: 'Charlie', allowed: true, reason: null, parameters: { efforts: ['low', 'high'], default_effort: 'low' } },
];
const setting = (key: string, value: unknown, revision: number): ResolvedSetting => ({ key, value, own: { value, revision, locked: false }, source: { scope: 'thread', node: null }, locked_by: null, writable: true, parts: [] });
const speedParameter = { key: 'service_tier', title: 'Speed', description: '', options: { from_model: 'service_tiers' } };
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
/** Presses a row once the list has read what it needs to write with. */
async function choose(user: ReturnType<typeof userEvent.setup>, name: string): Promise<void> {
  const row = await screen.findByRole('menuitemradio', { name });
  await waitFor(() => expect(row).toBeEnabled());
  await user.click(row);
}
function serve(models: ModelOption[], extra: Partial<ThreadParameters> = {}, current = 'alpha-routing', effort: string | null = 'high') {
  server.parameters.set(native.id, { model: { value: current, options: models, writable: true, source: { scope: 'thread', node: null } },
    effort: { value: effort, writable: true, supported: true, source: { scope: 'thread', node: null } }, parameters: [], approval_mode: 'on_request', approval_reviewer: null, ...extra });
}

beforeEach(() => {
  previousTheme = document.documentElement.getAttribute('data-theme');
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The model picker needs its native thread');
  native = { ...world.threads.thread(person, row), model: 'alpha-routing', model_display: 'Alpha', effort: 'high' };
  server.parameters.clear(); server.settings.clear(); server.write.mockReset(); server.choices.mockReset();
  serve(options);
  for (const one of [setting('agent.model', 'alpha-routing', 3), setting('agent.effort', 'high', 7), setting('agent.params', {}, 1)]) server.settings.set(one.key, one);
  server.write.mockImplementation((_id, key, body) => Promise.resolve(setting(key, body.value, (server.settings.get(key)?.own?.revision ?? 0) + 1)));
  server.choices.mockResolvedValue(undefined);
});
afterEach(async () => {
  cleanup(); threadWriter.evict(native.id);
  for (const style of styles.splice(0)) style.remove();
  if (previousTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', previousTheme);
  await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`lists the models alone as full-width single rows with one check mark in ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.setAttribute('data-theme', theme); stylesheet(theme === 'dark');
    show(<ModelEffort thread={native} open="model" />);
    const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.chip') });
    const menu = await within(picker).findByRole('menu', { name: i18n.t('composer.model.chip') });
    const rows = within(menu).getAllByRole('menuitemradio');
    expect(rows).toHaveLength(3);
    expect(within(menu).getByRole('menuitemradio', { name: 'Alpha', checked: true })).toBeVisible();
    expect(within(menu).getByRole('menuitemradio', { name: 'Bravo', checked: false })).toBeVisible();
    // The efforts are the effort control's, never nested under a model.
    expect(within(picker).queryByRole('menuitemradio', { name: effortDisplayName('low', i18n.t) })).not.toBeInTheDocument();
    expect(within(picker).queryByRole('group')).not.toBeInTheDocument();
    // What a row says beside its name: the provider and the context the model takes.
    expect(rows[0]?.closest('li')).toHaveTextContent(i18n.t('composer.model.window', { size: '400K' }));
    expect(menu).toHaveAttribute('aria-orientation', 'vertical'); expect(menu).toHaveClass('overflow-y-auto');
    for (const option of rows) {
      const choice = option.closest('[data-choice-layout]');
      expect(choice).toHaveAttribute('data-choice-layout', 'picker'); expect(choice).toHaveClass('w-full');
      expect(option).toHaveClass('w-full'); expect(option).not.toHaveAttribute('aria-pressed');
      expect(getComputedStyle(option).backgroundColor).toBe('rgba(0, 0, 0, 0)');
      expect(option.querySelectorAll('svg.lucide-check')).toHaveLength(option.getAttribute('aria-checked') === 'true' ? 1 : 0);
    }
    // Three models need no search.
    expect(within(picker).queryByRole('textbox')).not.toBeInTheDocument();
    expect(server.write).not.toHaveBeenCalled();
  });

  it(`lists the efforts of the chosen model alone with its default marked in ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
    show(<ModelEffort thread={native} open="effort" />);
    const picker = await screen.findByRole('dialog', { name: i18n.t('composer.effort.title') });
    const menu = await within(picker).findByRole('menu', { name: i18n.t('composer.effort.title') });
    expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(2);
    const high = within(menu).getByRole('menuitemradio', { name: effortDisplayName('high', i18n.t), checked: true });
    expect(high.closest('li')).toHaveTextContent(i18n.t('composer.effort.default'));
    expect(within(menu).getByRole('menuitemradio', { name: effortDisplayName('low', i18n.t), checked: false }).closest('li')).not.toHaveTextContent(i18n.t('composer.effort.default'));
    expect(within(picker).queryByRole('menuitemradio', { name: 'Bravo' })).not.toBeInTheDocument();
  });
}

it('shows the model, effort, speed and access program as chips that name their value alone', async () => {
  show(<ModelEffort thread={native} />);
  const model = await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.chip'), value: 'Alpha' }) });
  expect(model).toHaveTextContent('Alpha'); expect(model).not.toHaveTextContent(effortDisplayName('high', i18n.t));
  const effort = await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.effort.title'), value: effortDisplayName('high', i18n.t) }) });
  expect(effort).toHaveTextContent(effortDisplayName('high', i18n.t));
  // A model that lists no effort has no effort control.
  expect(document.querySelectorAll('[data-composer-chip="speed"]')).toHaveLength(0);
});

it('renders ten rows in the model list and six in the effort list for ten models with six efforts each, and searches the ten', async () => {
  const user = userEvent.setup();
  const ten: ModelOption[] = Array.from({ length: 10 }, (_, at) => ({ ...MODEL_ESTIMATE, model_name: `m${at}-routing`, display_name: `Model ${at}`, allowed: true, reason: null, parameters: { efforts: SIX, default_effort: 'medium' } }));
  serve(ten, {}, 'm3-routing', 'medium');
  const view = show(<ModelEffort thread={{ ...native, model: 'm3-routing', model_display: 'Model 3', effort: 'medium' }} open="model" />);
  const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.chip') });
  const menu = await within(picker).findByRole('menu', { name: i18n.t('composer.model.chip') });
  expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(10);
  const search = within(picker).getByRole('textbox', { name: i18n.t('composer.model.find') });
  await user.type(search, 'Model 7');
  expect(within(menu).getAllByRole('menuitemradio')).toHaveLength(1);
  view.rerender(<ModelEffort thread={{ ...native, model: 'm3-routing', model_display: 'Model 3', effort: 'medium' }} open="effort" />);
  const efforts = await screen.findByRole('menu', { name: i18n.t('composer.effort.title') });
  expect(within(efforts).getAllByRole('menuitemradio')).toHaveLength(6);
  expect(within(efforts).getByRole('menuitemradio', { name: effortDisplayName('medium', i18n.t), checked: true })).toBeVisible();
  expect(screen.queryByRole('textbox', { name: i18n.t('composer.model.find') })).not.toBeInTheDocument();
});

it('writes the effort alone when an effort is chosen, and the model with the effort it keeps when a model is', async () => {
  const user = userEvent.setup();
  const view = show(<ModelEffort thread={native} open="effort" />);
  await choose(user, effortDisplayName('low', i18n.t));
  await waitFor(() => expect(server.write).toHaveBeenCalledTimes(1));
  expect(server.write).toHaveBeenCalledWith(native.id, 'agent.effort', { value: 'low', expected_revision: 7 });
  server.write.mockClear();
  view.rerender(<ModelEffort thread={native} open="model" />);
  // Charlie takes the effort the thread is on, so the thread keeps it.
  await choose(user, 'Charlie');
  await waitFor(() => expect(server.write).toHaveBeenCalledTimes(2));
  expect(server.write).toHaveBeenNthCalledWith(1, native.id, 'agent.model', { value: 'charlie-routing', expected_revision: 3 });
  expect(server.write).toHaveBeenNthCalledWith(2, native.id, 'agent.effort', { value: 'high', expected_revision: 7 });
  server.write.mockClear();
  // Bravo lists none, and gets none.
  view.rerender(<ModelEffort thread={native} open="model" />);
  await choose(user, 'Bravo');
  await waitFor(() => expect(server.write).toHaveBeenCalledTimes(1));
  expect(server.write).toHaveBeenCalledWith(native.id, 'agent.model', { value: 'bravo-routing', expected_revision: 3 });
});

it('moves focus through the list without choosing, and chooses with the space bar', async () => {
  const user = userEvent.setup(); show(<ModelEffort thread={native} open="model" />);
  const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.chip') });
  const menu = await within(picker).findByRole('menu', { name: i18n.t('composer.model.chip') });
  const alpha = within(menu).getByRole('menuitemradio', { name: 'Alpha' }), bravo = within(menu).getByRole('menuitemradio', { name: 'Bravo' });
  const charlie = within(menu).getByRole('menuitemradio', { name: 'Charlie' });
  await waitFor(() => expect(alpha).toBeEnabled());
  expect(within(menu).getAllByRole('menuitemradio').filter((option) => option.tabIndex === 0)).toEqual([alpha]);
  alpha.focus();
  await user.keyboard('{ArrowDown}'); expect(bravo).toHaveFocus();
  await user.keyboard('{End}'); expect(charlie).toHaveFocus();
  await user.keyboard('{ArrowDown}'); expect(alpha).toHaveFocus();
  await user.keyboard('{ArrowUp}'); expect(charlie).toHaveFocus();
  expect(server.write).not.toHaveBeenCalled();
});

it('keeps every model read-only for a driver who cannot manage the thread, and says who can', async () => {
  const user = userEvent.setup();
  show(<ModelEffort thread={{ ...native, access: { ...native.access, permissions: native.access.permissions.filter((permission) => permission !== 'thread.manage') } }} open="model" />);
  const picker = await screen.findByRole('dialog', { name: i18n.t('composer.model.chip') });
  const menu = await within(picker).findByRole('menu', { name: i18n.t('composer.model.chip') });
  expect(within(picker).getByText(i18n.t('composer.model.readOnly', { who: native.owner.display_name }))).toBeVisible();
  for (const option of within(menu).getAllByRole('menuitemradio')) { expect(option).toBeDisabled(); await user.click(option); }
  expect(server.write).not.toHaveBeenCalled();
});

it('disables the controls of a thread that is only watched', async () => {
  show(<ModelEffort thread={native} readOnly />);
  expect(await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.chip'), value: 'Alpha' }) })).toBeDisabled();
  expect(server.write).not.toHaveBeenCalled();
});

for (const compact of [false, true]) {
  it(`keeps the effort beside a long model name instead of folding it into the name in narrow:${compact}`, async () => {
    const name = 'A long actual model display name';
    show(<ViewCompactContext.Provider value={compact}><ModelEffort thread={{ ...native, model_display: name }} /></ViewCompactContext.Provider>);
    const effort = effortDisplayName('high', i18n.t);
    expect(await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.effort.title'), value: effort }) })).toBeVisible();
    expect(screen.getByRole('button', { name: new RegExp(i18n.t('composer.model.chip')) })).not.toHaveTextContent(`(${effort})`);
  });
}

it('shows a model that offers exactly two speeds as one chip that says the speed that is on, and flips it', async () => {
  const user = userEvent.setup();
  const fast = { ...options[0]!, parameters: { ...options[0]!.parameters, service_tiers: [{ id: 'fast', label: 'Fast' }] } };
  const view = { apply: 'next_turn' as const, def: speedParameter, key: 'service_tier', locked_by: null, source: { scope: 'default' as const, node: null }, value: null, writable: true };
  serve([fast, ...options.slice(1)], { parameters: [view] });
  show(<ModelEffort thread={native} />);
  const chip = await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.speed.title'), value: i18n.t('composer.model.speed.standard') }) });
  expect(chip).toHaveAttribute('aria-pressed', 'false');
  await waitFor(() => expect(chip).not.toHaveAttribute('aria-disabled'));
  await user.click(chip);
  await waitFor(() => expect(server.choices).toHaveBeenCalledTimes(1));
  const [thread, change] = server.choices.mock.calls[0] ?? [];
  expect(thread).toBe(native.id);
  expect(change?.({})).toEqual({ service_tier: 'fast' });
});

it('shows no speed control for a model that offers one speed only', async () => {
  const view = { apply: 'next_turn' as const, def: speedParameter, key: 'service_tier', locked_by: null, source: { scope: 'default' as const, node: null }, value: null, writable: true };
  serve(options, { parameters: [view] });
  show(<ModelEffort thread={native} />);
  await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.chip'), value: 'Alpha' }) });
  expect(document.querySelectorAll('[data-composer-chip="speed"]')).toHaveLength(0);
});

it('lists more than two speeds in a list of their own', async () => {
  const user = userEvent.setup();
  const tiers = [{ id: 'fast', label: 'Fast' }, { id: 'flex', label: 'Flex' }];
  const many = { ...options[0]!, parameters: { ...options[0]!.parameters, service_tiers: tiers } };
  const view = { apply: 'next_turn' as const, def: speedParameter, key: 'service_tier', locked_by: null, source: { scope: 'default' as const, node: null }, value: null, writable: true };
  serve([many, ...options.slice(1)], { parameters: [view] });
  show(<ModelEffort thread={native} open="speed" />);
  const menu = await screen.findByRole('menu', { name: i18n.t('composer.model.speed.title') });
  expect(within(menu).getAllByRole('menuitemradio').map((row) => row.textContent)).toEqual([expect.stringContaining('Standard'), expect.stringContaining('Fast'), expect.stringContaining('Flex')]);
  await choose(user, 'Flex');
  await waitFor(() => expect(server.choices).toHaveBeenCalledTimes(1));
});
