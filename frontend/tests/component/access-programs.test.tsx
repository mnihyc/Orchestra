/** Provider and thread choices name programs for people while retaining their vendor identities. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { AccessChoice } from '@/features/composer/SpeedAndAccess';
import { statesOf, type SpeedOrAccess } from '@/features/composer/modelState';
import { EntryFields } from '@/features/resources/providers/ModelEditor';
import { Summary } from '@/features/resources/providers/Models';
import { formOf, parametersOf, type EntryForm } from '@/features/resources/providers/entry';
import type { ModelParameters, ThreadParameters } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/resources/harnesses', () => ({ useHarnessTitle: () => () => 'Codex' }));

const entry: ModelParameters = { offered: { access_programs: ['daybreakBlue', 'daybreakRed', 'standard', 'daybreakGreen'] }, access_programs: ['daybreakBlue'] };
afterEach(async () => { await setLocale('en'); });

function Editor({ changed }: { changed: (form: EntryForm) => void }) {
  const [form, setForm] = useState(() => formOf(entry));
  return <EntryFields base={entry} form={form} onChange={(next) => { setForm(next); changed(next); }} faults={{}} disabled={false} otherGroups={[]} provider={{ kind: 'api', harness_kind: null }} />;
}

for (const locale of ['en', 'zh-CN'] as const) it(`names the provider's offered programs and writes their ids (${locale})`, async () => {
  await setLocale(locale);
  const changed = vi.fn<(form: EntryForm) => void>();
  const user = userEvent.setup();
  show(<Editor changed={changed} />);
  const group = screen.getByRole('group', { name: i18n.t('models.editor.programs.title') });
  const blue = locale === 'en' ? 'Daybreak Blue' : 'Daybreak 蓝';
  const red = locale === 'en' ? 'Daybreak Red' : 'Daybreak 红';
  const standard = locale === 'en' ? 'Standard' : '标准';
  expect(within(group).getByRole('switch', { name: blue })).toBeChecked();
  expect(within(group).getByRole('switch', { name: 'daybreakGreen' })).not.toBeChecked();
  await user.click(within(group).getByRole('switch', { name: red }));
  await user.click(within(group).getByRole('switch', { name: standard }));
  await user.click(within(group).getByRole('combobox', { name: i18n.t('models.editor.startsWith') }));
  await user.click(screen.getByRole('option', { name: standard }));
  const form = changed.mock.lastCall?.[0];
  if (form === undefined) throw new Error('The selected program did not update the model form');
  expect(parametersOf(form, entry)).toMatchObject({ access_programs: ['daybreakBlue', 'daybreakRed', 'standard'], default_access_program: 'standard' });
});

for (const locale of ['en', 'zh-CN'] as const) it(`shows and selects thread programs by name while sending their ids (${locale})`, async () => {
  await setLocale(locale);
  const parameters = {
    model: { value: 'model', options: [{ model_name: 'model', parameters: { access_programs: ['daybreakBlue', 'standard', 'daybreakGreen'] } }] },
    parameters: [{ key: 'access', def: { key: 'access', options: { from_model: 'access_programs' } }, value: 'daybreakBlue', source: { scope: 'thread', node: null }, locked_by: null, writable: true, apply: 'next_turn' }],
  } as unknown as ThreadParameters;
  const state = statesOf(parameters, { access: 'daybreakBlue' }).find((one): one is SpeedOrAccess => one.kind === 'access');
  if (state === undefined) throw new Error('The model did not produce an access choice');
  const choose = vi.fn();
  show(<AccessChoice state={state} writable chooser={null} onChoose={choose} />);
  const picker = screen.getByRole('combobox', { name: i18n.t('composer.model.access.title') });
  expect(picker).toHaveTextContent(locale === 'en' ? 'Daybreak Blue' : 'Daybreak 蓝');
  await userEvent.click(picker);
  expect(screen.getByRole('option', { name: 'daybreakGreen' })).toBeVisible();
  await userEvent.click(screen.getByRole('option', { name: locale === 'en' ? 'Standard' : '标准' }));
  expect(choose).toHaveBeenCalledWith('standard');
});

for (const locale of ['en', 'zh-CN'] as const) it(`names enabled and offered programs in the provider's model summary (${locale})`, async () => {
  await setLocale(locale);
  const { container } = show(<Summary parameters={entry} />);
  expect(container).toHaveTextContent(locale === 'en' ? 'Daybreak Blue' : 'Daybreak 蓝');
  expect(container).toHaveTextContent(locale === 'en' ? 'Daybreak Red' : 'Daybreak 红');
  expect(container).toHaveTextContent('daybreakGreen');
  expect(container).not.toHaveTextContent('daybreakBlue');
  expect(container).not.toHaveTextContent('daybreakRed');
});
