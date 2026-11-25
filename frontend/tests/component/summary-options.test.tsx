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
  const automatic = (target: string) => screen.getByRole('switch', { name: `${i18n.t(`summaryOptions.fields.${target}`)}: ${i18n.t('settingsEditors.targets.automatic')}` });
  expect(automatic('document')).not.toBeChecked();
  await user.click(automatic('document'));
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: true } });
  // The events are named in the reader's words, as chips of the row.
  const row = within(document.querySelector<HTMLElement>('[data-summary-target="document"]') ?? document.body);
  expect(row.getByRole('button', { name: i18n.t('choiceCatalog.automationTrigger.content_changed.name') })).toBeVisible();
  const output = row.getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.output_chars') });
  await user.type(output, '1200');
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: true, output_chars: 1200 } });
  // Written and cached targets have no model knobs: they have no row.
  for (const field of ['help', 'search_results'] as const) expect(document.querySelector(`[data-summary-target="${field}"]`)).toBeNull();
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
  const automatic = (target: string) => screen.getByRole('switch', { name: `${i18n.t(`summaryOptions.fields.${target}`)}: ${i18n.t('settingsEditors.targets.automatic')}` });
  expect(automatic('chat_title')).not.toBeChecked();
  await user.click(automatic('chat_title'));
  const title = within(document.querySelector<HTMLElement>('[data-summary-target="chat_title"]') ?? document.body);
  await user.type(title.getByRole('spinbutton', { name: i18n.t('settingsEditors.targets.output_chars') }), '80');
  expect(changed).toHaveBeenLastCalledWith({ chat: { automatic: true }, chat_title: { automatic: true, output_chars: 80 } });
  expect(automatic('chat')).toBeChecked();
});
