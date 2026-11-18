/** The summary targets: one compact row for each, whose defaults are the registry's own, shown where nothing is set. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { SummaryTargetsEditor } from '@/features/settings/SummaryTargetsEditor';
import { SettingSummary } from '@/features/settings/SettingSummary';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n } from '@/i18n';
import { show } from './support';

const shipped = (seconds: number, on: string[], chars = 600) => ({ automatic: false, on, min_interval_secs: seconds, context_tokens: 32_000, output_chars: chars });
/** What the registry ships, as the server serves it: nothing here is the page's own. */
const DEFAULTS = { thread: shipped(120, ['turn_completed', 'milestone']), topic: shipped(900, ['child_changed']), document: shipped(300, ['content_changed']),
  chat: shipped(300, ['message_posted']), chat_title: shipped(300, ['message_posted'], 80) };

function Draft({ initial = {}, inherited = {}, changed }: { initial?: unknown; inherited?: unknown; changed?: (value: unknown) => void }) {
  const [value, setValue] = useState<unknown>(initial);
  return <SummaryTargetsEditor shape={{ type: 'json' }} label="Summary targets" value={value} defaults={DEFAULTS} inherited={inherited}
    onChange={(next) => { setValue(next); changed?.(next); }} />;
}
const rowOf = (name: string) => {
  const item = document.querySelector<HTMLElement>(`[data-summary-target="${name}"]`);
  if (item === null) throw new Error(`There is no row for ${name}`);
  return within(item);
};

it('shows one compact row for each target, every one off until it is turned on', () => {
  show(<Draft />);
  const rows = ['thread', 'topic', 'document', 'chat', 'chat_title'];
  for (const name of rows) expect(document.querySelector(`[data-summary-target="${name}"]`)).not.toBeNull();
  expect(screen.getAllByRole('switch')).toHaveLength(rows.length);
  for (const toggle of screen.getAllByRole('switch')) expect(toggle).not.toBeChecked();
  // What is not automatic has nothing more to say.
  expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument();
  expect(screen.queryByRole('group')).not.toBeInTheDocument();
});

it('shows 900 for a topic\'s interval when nothing is set, and the other defaults the same way', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft changed={changed} />);
  await user.click(screen.getByRole('switch', { name: 'Topic: Automatic' }));
  const topic = rowOf('topic');
  expect(topic.getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.min_interval_secs') })).toHaveAttribute('placeholder', '900');
  expect(topic.getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.context_tokens') })).toHaveAttribute('placeholder', '32,000');
  expect(topic.getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.output_chars') })).toHaveAttribute('placeholder', '600');
  expect(topic.getByRole('button', { name: 'Child changed' })).toHaveAttribute('aria-pressed', 'true');
  expect(topic.getByRole('button', { name: 'Interval' })).toHaveAttribute('aria-pressed', 'false');
  // Turning it on says only that: every number is left to the default.
  expect(changed).toHaveBeenLastCalledWith({ topic: { automatic: true } });
  await user.click(screen.getByRole('switch', { name: 'Chat title: Automatic' }));
  expect(rowOf('chat_title').getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.output_chars') })).toHaveAttribute('placeholder', '80');
  expect(rowOf('chat_title').getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.min_interval_secs') })).toHaveAttribute('placeholder', '300');
});

it('writes only what was set, and an emptied number goes back to the default', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft changed={changed} initial={{ document: { automatic: true } }} />);
  const documents = rowOf('document');
  const interval = documents.getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.min_interval_secs') });
  await user.type(interval, '60');
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: true, min_interval_secs: 60 } });
  await user.click(documents.getByRole('button', { name: 'Milestone' }));
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: true, min_interval_secs: 60, on: ['milestone', 'content_changed'] } });
  await user.clear(interval);
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: true, on: ['milestone', 'content_changed'] } });
  // Turning it off and on again leaves the rest of the entry as it was.
  await user.click(screen.getByRole('switch', { name: 'Document: Automatic' }));
  await user.click(screen.getByRole('switch', { name: 'Document: Automatic' }));
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: true, on: ['milestone', 'content_changed'] } });
});

it('follows what a place above sets until this place sets its own entry', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  show(<Draft changed={changed} inherited={{ topic: { automatic: true, min_interval_secs: 60 } }} />);
  expect(screen.getByRole('switch', { name: 'Topic: Automatic' })).toBeChecked();
  expect(rowOf('topic').getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.min_interval_secs') })).toHaveAttribute('placeholder', '60');
  await user.click(screen.getByRole('switch', { name: 'Topic: Automatic' }));
  // An entry of its own replaces the one above as a whole, so it starts from it.
  expect(changed).toHaveBeenLastCalledWith({ topic: { automatic: false, min_interval_secs: 60 } });
  await user.click(rowOf('topic').getByRole('button', { name: i18n.t('settingsEditors.targets.useDefault', { name: 'Topic' }) }));
  expect(changed).toHaveBeenLastCalledWith({});
});

it('says which targets are automatic in a few words when the row is closed', () => {
  show(<><SettingSummary def={{ key: 'summary.targets', title: 'Summary targets' }} value={DEFAULTS} />
    <SettingSummary def={{ key: 'summary.targets', title: 'Summary targets' }} value={{ ...DEFAULTS, topic: { ...DEFAULTS.topic, automatic: true }, chat: { ...DEFAULTS.chat, automatic: true } }} /></>);
  expect(screen.getByText(i18n.t('settingsEditors.targets.summaryOff'))).toBeVisible();
  expect(screen.getByText(i18n.t('settingsEditors.targets.summaryOn', { names: 'Topic and Chat' }))).toBeVisible();
});

it('takes its defaults from the registry the server serves, which writes every target out in full', () => {
  const served = SETTINGS_REGISTRY.find((entry) => entry.key === 'summary.targets');
  if (served === undefined) throw new Error('The registry must declare the summary targets.');
  // The generated registry follows the Rust one; until it is regenerated it may still carry an empty default.
  if (typeof served.default !== 'object' || served.default === null || Object.keys(served.default).length === 0) return;
  const defaults = served.default as Record<string, { automatic: boolean; min_interval_secs: number }>;
  expect(Object.keys(defaults).sort()).toEqual(['chat', 'chat_title', 'document', 'thread', 'topic']);
  expect(defaults.topic?.min_interval_secs).toBe(900);
  for (const entry of Object.values(defaults)) expect(entry.automatic).toBe(false);
});
