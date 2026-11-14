/** Opting into a shipped summary target retains its displayed required false value on later edits. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { shapeOf } from '@/features/settings/settingsModel';
import { ValueControl } from '@/features/settings/ValueControl';
import { SummarizerChoice } from '@/features/settings/SummarizerChoice';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`chooses a registered summarizer and edits its fields without writing JSON in ${locale}`, async () => {
  await setLocale(locale);
  const user = userEvent.setup(), changed = vi.fn();
  const schema = { type: 'object', properties: { kind: { type: 'string' } }, additionalProperties: true,
    'x-summarizers': [{ name: 'rules', title: 'Rule summaries', fields: ['summary'], config_schema: {
      type: 'object', properties: { prefix: { type: 'string', default: 'Summary:' } }, required: ['prefix'], additionalProperties: false,
    } }] };
  function Editor() {
    const [value, setValue] = useState<unknown>({});
    const props = { schema, shape: shapeOf(schema), value, label: 'Producer', onChange: (next: unknown) => { changed(next); setValue(next); } };
    return <SummarizerChoice {...props} />;
  }
  show(<Editor />);
  await user.click(screen.getByRole('combobox', { name: 'Producer' }));
  await user.click(screen.getByRole('option', { name: 'Rule summaries' }));
  const prefix = screen.getByRole('textbox', { name: 'Prefix' });
  expect(prefix).toHaveValue('Summary:');
  await user.clear(prefix); await user.type(prefix, 'Review:');
  expect(changed).toHaveBeenLastCalledWith({ kind: 'rules', prefix: 'Review:' });
  expect(screen.queryByRole('textbox', { name: 'Producer' })).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) it(`includes the untouched automatic=false member of a new document target in ${locale}`, async () => {
  await setLocale(locale);
  const user = userEvent.setup(), changed = vi.fn();
  const definition = SETTINGS_REGISTRY.find((entry) => entry.key === 'summary.targets');
  if (definition === undefined) throw new Error('The shipped summary target definition is required.');
  const all = shapeOf(definition.schema);
  if (all.type !== 'object') throw new Error('Summary targets have a structured schema.');
  const shape = { ...all, members: all.members.filter((member) => member.name === 'document') };
  const labels: Record<string, string> = { document: i18n.t('summaryOptions.fields.document'), automatic: i18n.t('summaryOptions.fields.automatic'), output_chars: i18n.t('summaryOptions.fields.output_chars') };
  function Editor() {
    const [value, setValue] = useState<unknown>({});
    return <ValueControl shape={shape} value={value} label="Summary targets" memberLabel={(name) => labels[name] ?? name}
      onChange={(next) => { changed(next); setValue(next); }} />;
  }
  show(<Editor />);
  const document = screen.getByRole('checkbox', { name: i18n.t('settings.editor.memberSet', { name: labels.document }) });
  await user.click(document);
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: false } });
  expect(screen.getByRole('switch', { name: labels.automatic })).not.toBeChecked();
  await user.click(screen.getByRole('checkbox', { name: i18n.t('settings.editor.memberSet', { name: labels.output_chars }) }));
  const output = screen.getByRole('spinbutton', { name: labels.output_chars });
  await user.clear(output); await user.type(output, '1200');
  expect(changed).toHaveBeenLastCalledWith({ document: { automatic: false, output_chars: 1200 } });
  await user.click(document);
  expect(changed).toHaveBeenLastCalledWith({});
});
