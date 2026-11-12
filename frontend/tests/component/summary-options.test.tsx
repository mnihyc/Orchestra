/** One schema form edits generated-target options; written/cached targets offer no model knobs. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { SettingValue } from '@/features/settings/SettingValue';
import { shapeOf } from '@/features/settings/settingsModel';
import { localizedSetting } from '@/features/settings/settingText';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`edits document defaults with the shared control and localized event names (${locale})`, async () => {
  await setLocale(locale);
  const source = SETTINGS_REGISTRY.find((entry) => entry.key === 'summary.targets');
  if (source === undefined) throw new Error('Generate the summary target registry first');
  const def = localizedSetting(source, i18n);
  const changed = vi.fn<(value: unknown) => void>();
  function Form() {
    const [value, setValue] = useState<unknown>({});
    return <SettingValue def={def} shape={shapeOf(def.schema)} value={value} label={def.title}
      target={{ kind: 'node', id: 'topic' }} setting={{ key: def.key, value: {}, own: null, writable: true, locked_by: null, source: { scope: 'default', node: null }, parts: [] }}
      onChange={(next) => { changed(next); setValue(next); }} />;
  }
  show(<Form />);
  const user = userEvent.setup();
  const set = (name: string) => i18n.t('settings.editor.memberSet', { name });
  await user.click(screen.getByRole('checkbox', { name: set(i18n.t('summaryOptions.fields.document')) }));
  const automatic = screen.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') });
  expect(automatic).not.toBeChecked();
  await user.click(automatic);
  await user.click(screen.getByRole('checkbox', { name: set(i18n.t('summaryOptions.fields.on')) }));
  await user.click(screen.getByRole('checkbox', { name: i18n.t('choiceCatalog.automationTrigger.content_changed.name') }));
  await user.click(screen.getByRole('checkbox', { name: set(i18n.t('summaryOptions.fields.output_chars')) }));
  const output = screen.getByRole('spinbutton', { name: i18n.t('summaryOptions.fields.output_chars') });
  await user.clear(output); await user.type(output, '1200');
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: true, on: ['content_changed'], output_chars: 1200 } });
  for (const field of ['help', 'search_results'] as const) expect(screen.queryByRole('checkbox', { name: set(i18n.t(`summaryOptions.fields.${field}`)) })).not.toBeInTheDocument();
});


for (const locale of ['en', 'zh-CN'] as const) it(`edits a chat title independently of catch-up in the generated schema (${locale})`, async () => {
  await setLocale(locale);
  const source = SETTINGS_REGISTRY.find((entry) => entry.key === 'summary.targets');
  if (source === undefined) throw new Error('Generate the summary target registry first');
  const def = localizedSetting(source, i18n);
  const changed = vi.fn<(value: unknown) => void>();
  function Form() {
    const [value, setValue] = useState<unknown>({ chat: { automatic: true } });
    return <SettingValue def={def} shape={shapeOf(def.schema)} value={value} label={def.title}
      target={{ kind: 'node', id: 'topic' }} setting={{ key: def.key, value: {}, own: null, writable: true, locked_by: null, source: { scope: 'default', node: null }, parts: [] }}
      onChange={(next) => { changed(next); setValue(next); }} />;
  }
  show(<Form />);
  const user = userEvent.setup();
  const set = (name: string) => i18n.t('settings.editor.memberSet', { name });
  await user.click(screen.getByRole('checkbox', { name: set(i18n.t('summaryOptions.fields.chat_title')) }));
  const title = within(screen.getByRole('group', { name: i18n.t('summaryOptions.fields.chat_title') }));
  expect(title.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') })).not.toBeChecked();
  await user.click(title.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') }));
  await user.click(title.getByRole('checkbox', { name: set(i18n.t('summaryOptions.fields.on')) }));
  await user.click(title.getByRole('checkbox', { name: i18n.t('choiceCatalog.automationTrigger.message_posted.name') }));
  await user.click(title.getByRole('checkbox', { name: set(i18n.t('summaryOptions.fields.output_chars')) }));
  const output = title.getByRole('spinbutton', { name: i18n.t('summaryOptions.fields.output_chars') });
  await user.clear(output); await user.type(output, '80');
  expect(changed).toHaveBeenLastCalledWith({ chat: { automatic: true }, chat_title: { automatic: true, on: ['message_posted'], output_chars: 80 } });
  expect(within(screen.getByRole('group', { name: i18n.t('summaryOptions.fields.chat') })).getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') })).toBeChecked();
});
