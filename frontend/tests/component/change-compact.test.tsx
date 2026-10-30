/** Short names; choice consequences belong to tooltips and the field's one question. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { initialChoices, requestOf, selectionOf, type Choices, type FileChoice } from '@/features/change/choices';
import { FilesChoice } from '@/features/change/Files';
import { HistoryChoices } from '@/features/change/History';
import { Options } from '@/features/change/Options';
import { HostChoices, Where } from '@/features/change/Where';
import type { EnvironmentRef, ForkPlan, ForkPlanHistory, Place, PlaceEnvironment, PlaceHost, Thread, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';

import { show } from './support';

const fixture = vi.hoisted(() => ({ places: [] as Place[], summary: true, history: 'carry' }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), placesQuery: (topic: string) => ({
  queryKey: ['compact-places', topic], initialData: fixture.places, queryFn: () => Promise.resolve(fixture.places),
}) }));
vi.mock('@/api/settings', async (original) => ({ ...await original<object>(), settingsQuery: (thread: string) => ({
  queryKey: ['compact-summary', thread], queryFn: () => Promise.resolve([
    { key: 'auto.summarizer', value: fixture.summary ? { kind: 'rules' } : null }, { key: 'thread.switch_history', value: fixture.history },
  ]),
}) }));
vi.mock('@/realtime/stores/thread', async (original) => ({ ...await original<object>(), useThreadView: () => ({
  order: [], items: new Map(), turns: new Map(),
}) }));

const owner: UserRef = { id: 'owner', kind: 'human', username: 'owner', display_name: 'Other Owner', avatar: { color: 1, text: 'O' } };
const environment: EnvironmentRef = { id: 'other', name: 'Other workspace', address: 'team.owner.other', placement: 'connected', status: 'running', owner, ports_exposed: false };
const place: PlaceEnvironment = {
  environment, host: null, machine: { id: 'workstation', name: 'Workstation', online: true, placement: 'connected', platform_os: 'linux' },
  facts: { os: 'linux', cpus: 4, memory_mb: 8192 }, threads: 2, topics: [], topics_hidden: 0, mounts: [], usable: true, reasons: [], current: false,
};
// This editor reads only topic_id and the source environment; no unrelated thread fact is fabricated.
const thread = { topic_id: 'topic', environment: { ...environment, id: 'source', name: 'Source workspace' } } as unknown as Thread;

beforeEach(() => {
  const declared = { ...place, kind: 'environment' as const };
  fixture.places = [declared];
  fixture.summary = true;
  fixture.history = 'carry';
});
const appearance = useAppearance.getState();
afterEach(async () => { useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  const words = locale === 'en' ? {
    all: 'All files', patterns: 'By pattern', none: 'No files', include: 'Take these',
    shared: 'Source workspace · share files', existing: 'Copy to existing', fresh: 'Copy to new', picker: 'Another environment', environment: 'Environment', historyLabel: 'History',
    original: 'Keep thread access', topic: 'Use topic access', private: 'Private to me',
    history: 'Carry', range: 'Carry some turns', summary: 'Summary', noHistory: 'None',
  } : {
    all: '所有文件', patterns: '按规则选择', none: '不带文件', include: '带上这些',
    shared: 'Source workspace · 共用文件', existing: '复制到已有环境', fresh: '复制到新环境', picker: '另一个环境', environment: '环境', historyLabel: '历史',
    original: '沿用会话权限', topic: '使用话题权限', private: '仅我可用',
    history: '延续', range: '延续部分轮次', summary: '摘要', noHistory: '不带历史',
  };

  it(`file names explain their effect without description rows (${locale})`, async () => {
    await setLocale(locale);
    const sent = vi.fn();
    function Form() {
      const [files, setFiles] = useState<FileChoice>({ mode: 'all' });
      return <FilesChoice value={files} onChange={(next) => { setFiles(next); sent(selectionOf(next)); }} />;
    }
    const { container } = show(<Form />);
    const user = userEvent.setup();
    expect(screen.getAllByRole('radio').map((option) => option.textContent)).toEqual([words.all, words.patterns, words.none]);
    expect(container.querySelector('[data-choice-descriptions]')).toBeNull();
    expect(container.querySelector('[data-choice-help]')).toBeNull();
    await user.click(screen.getByRole('radio', { name: words.patterns }));
    await user.type(screen.getByRole('textbox', { name: words.include }), 'src/**');
    expect(sent).toHaveBeenLastCalledWith({ mode: 'selection', include: ['src/**'] });
    await user.click(screen.getByRole('radio', { name: words.none }));
    expect(sent).toHaveBeenLastCalledWith({ mode: 'none' });
  });

  it(`destination names say share or copy and inventory details stay in the dropdown (${locale})`, async () => {
    await setLocale(locale);
    const sent = vi.fn();
    function Form() {
      const [choices, setChoices] = useState(initialChoices('new'));
      return <Where thread={thread} fork choices={choices} onChange={(patch) => setChoices((current) => {
        const next = { ...current, ...patch }; sent(requestOf(next)); return next;
      })} />;
    }
    show(<Form />);
    const user = userEvent.setup();
    const destination = screen.getByRole('combobox', { name: words.environment });
    expect(destination).toHaveTextContent(words.shared);
    await user.click(destination);
    expect(screen.getByRole('option', { name: words.fresh })).toBeVisible();
    await user.click(screen.getByRole('option', { name: words.existing }));
    const picker = await screen.findByRole('combobox', { name: words.picker });
    expect(picker).toHaveTextContent('Other workspace');
    expect(screen.queryByText(/Other Owner/)).toBeNull();
    await user.click(picker);
    expect(screen.getByRole('option', { name: /Other workspace/ })).toBeVisible();
    expect(screen.getByText(/Other Owner/)).toBeVisible();
    await user.keyboard('{Escape}');
    expect(screen.queryByText(/Other Owner/)).toBeNull();
    expect(sent).toHaveBeenLastCalledWith(expect.objectContaining({ environment: { use: 'other' } }));
  });

  it(`keeps access consequences visible beside each form choice (${locale})`, async () => {
    await setLocale(locale);
    const sent = vi.fn();
    function Form() {
      const [choices, setChoices] = useState(initialChoices('new'));
      return <Options choices={choices} applied={null} plan={null} topic="Design" onChange={(patch) => setChoices((current) => {
        const next = { ...current, ...patch }; sent(requestOf(next)); return next;
      })} />;
    }
    const { container } = show(<Form />);
    const user = userEvent.setup();
    expect(screen.getByRole('radio', { name: words.original })).toBeChecked();
    expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
    expect(container.querySelector('[data-choice-descriptions]')).toBeNull();
    await user.click(screen.getByRole('radio', { name: words.topic }));
    expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
    expect(screen.getByRole('radio', { name: words.topic })).toHaveAccessibleDescription(i18n.t('changeCompact.accessFromTopic', { topic: 'Design' }));
    expect(sent).toHaveBeenLastCalledWith(expect.objectContaining({ access: 'defaults' }));
    await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('fork.page.title') }) }));
    const help = screen.getByRole('dialog', { name: i18n.t('fork.page.title') });
    expect(help).toHaveTextContent(i18n.t('fork.page.originalAbout'));
    expect(help).toHaveTextContent(i18n.t('changeCompact.accessFromTopic', { topic: 'Design' }));
    expect(help).toHaveTextContent(i18n.t('fork.page.privateAbout'));
    expect(within(help).getAllByRole('term')).toHaveLength(3);
    expect(sent).toHaveBeenCalledTimes(1);
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('radio', { name: words.private }));
    expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
    expect(sent).toHaveBeenLastCalledWith(expect.objectContaining({ access: 'private' }));
  });

  it(`history names stay compact and preserve the summary gate (${locale})`, async () => {
    await setLocale(locale);
    const changed = vi.fn();
    const choices = initialChoices('new');
    const { container } = show(<HistoryChoices threadId="thread" choices={choices} onChange={changed} />);
    const history = screen.getByRole('combobox', { name: words.historyLabel });
    expect(history).toHaveTextContent(words.history);
    await userEvent.click(history);
    expect(await screen.findByRole('option', { name: words.summary })).not.toHaveAttribute('aria-disabled', 'true');
    expect(container.querySelector('[data-choice-descriptions]')).toBeNull();
    await userEvent.click(screen.getByRole('option', { name: words.range }));
    expect(changed).toHaveBeenCalledWith({ history: { mode: 'range', fromTurn: null, toTurn: null } });
  });

  for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) it(`names the checked native-import limitation beside Carry while preserving neutral import (${locale}/${theme}/${width})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); Object.defineProperty(window, 'innerWidth', { configurable: true, value: width });
    const named = { harness: { kind: 'codex', name: 'Codex', names: {} } };
    const planned: ForkPlanHistory = {
      mode: 'all', turns: 2, method: 'neutral_import', source_available: true,
      archive_bytes: 512, native_import_unavailable: { ...named, kind: 'codex', version: '0.162.1', build: null },
      report: { fidelity: 'structured', items_total: 2, by_kind: {}, dropped: [], warnings: [] },
    };
    const choices = initialChoices('replace');
    const view = show(<HistoryChoices threadId="thread" choices={choices} planned={planned} onChange={vi.fn()} />);
    const reason = i18n.t('programVersions.featureOff', { feature: i18n.t('programVersions.disabledFeatures.nativeImport'), name: 'Codex', version: '0.162.1' });
    expect(screen.getByRole('status')).toHaveTextContent(i18n.t('programVersions.neutralCarry', { reason }));
    expect(screen.getByRole('combobox', { name: words.historyLabel })).toHaveTextContent(words.history);
    expect(screen.getByRole('combobox', { name: words.historyLabel })).toBeEnabled();
    view.rerender(<HistoryChoices threadId="thread" choices={{ ...choices, history: { ...choices.history, mode: 'none' } }} planned={planned} onChange={vi.fn()} />);
    expect(screen.queryByText(reason)).not.toBeInTheDocument();
  });

  it(`without a summarizer a summary can still be chosen and carries the last turns briefly (${locale})`, async () => {
    await setLocale(locale);
    fixture.summary = false;
    const changed = vi.fn();
    show(<HistoryChoices threadId="thread" choices={initialChoices('replace')} onChange={changed} />);
    await userEvent.click(screen.getByRole('combobox', { name: words.historyLabel }));
    const summary = await screen.findByRole('option', { name: words.summary });
    expect(summary).not.toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(summary);
    expect(changed).toHaveBeenCalledWith({ history: { mode: 'summary', fromTurn: null, toTurn: null } });
  });

  it(`shows what the place carries until the person chooses (${locale})`, async () => {
    await setLocale(locale);
    fixture.history = 'none';
    show(<HistoryChoices threadId="thread" choices={initialChoices('replace')} onChange={vi.fn()} />);
    const history = screen.getByRole('combobox', { name: words.historyLabel });
    // Until the place's settings are read it shows Carry, which is what ships; then the place's own choice.
    await waitFor(() => expect(history).toHaveTextContent(words.noHistory));
  });
}

it('keeps host capacity and policy refusals inside the open picker without weakening the gate', async () => {
  const host: PlaceHost = {
    host: { id: 'host', name: 'Build host', address: 'team.owner.build' }, owner, labels: [],
    capacity: { cpu_millis: 4000, memory_mb: 8192, disk_gb: 100 }, declared: { cpu_millis: 1000, memory_mb: 2048, disk_gb: 10 },
    free: { cpu_millis: 3000, memory_mb: 6144, disk_gb: 90 }, used: { cpu_percent: 5, memory_mb: 1024, disk_gb: 8 },
    environments: { running: 1, idle: 0 }, storages: [], mounts: [], when_full: 'block', usable: false,
    reasons: [], policy_refusal: 'Machine policy forbids writes.', warnings: [], current: false, automatic: false,
  };
  const changed = vi.fn();
  show(<HostChoices hosts={[host]} value={null} loading={false} onChange={changed} />);
  expect(screen.queryByText('Machine policy forbids writes.')).toBeNull();
  expect(screen.queryByRole('meter')).toBeNull();
  await userEvent.click(screen.getByRole('combobox', { name: 'Place' }));
  const option = screen.getByRole('option', { name: 'Build host' });
  expect(option).toHaveAttribute('aria-disabled', 'true');
  expect(option).toHaveAccessibleDescription(/Machine policy forbids writes/);
  expect(screen.getByRole('meter', { name: '2 of 8 GB declared' })).toBeVisible();
  await userEvent.click(option);
  expect(changed).not.toHaveBeenCalled();
});

it('keeps removal consequences beside the option and preserves the exact plan choices', async () => {
  const sent = vi.fn();
  // Only the environment-leaving fact is consumed by Options; no job or plan is started here.
  const plan = { leaves: { environment: { action: 'in_use' } }, memory: null } as unknown as ForkPlan;
  function Form() {
    const [choices, setChoices] = useState<Choices>(initialChoices('replace'));
    return <Options choices={choices} applied={null} plan={plan} topic="Design" onChange={(patch) => setChoices((current) => {
      const next = { ...current, ...patch }; sent(requestOf(next)); return next;
    })} />;
  }
  const { container } = show(<Form />);
  expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
  const user = userEvent.setup();
  const remove = screen.getByRole('radio', { name: 'Remove old environment' });
  expect(remove).toHaveAccessibleDescription(i18n.t('choiceCatalog.oldRuntime.remove.description'));
  await user.click(screen.getByRole('radio', { name: 'Keep old environment' }));
  expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
  await user.click(screen.getByRole('radio', { name: 'Leave processes running' }));
  const interrupt = screen.getByRole('radio', { name: 'Interrupt turn' });
  expect(interrupt).not.toBeChecked();
  await user.click(interrupt);
  expect(interrupt).toBeChecked();
  expect(sent).toHaveBeenLastCalledWith(expect.objectContaining({ options: { keep_previous: true, stop_processes: false, active_turn: 'interrupt' } }));
});
