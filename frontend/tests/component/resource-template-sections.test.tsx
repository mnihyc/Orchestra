/** Bounded template sections retain the actual schema form, validation, sources and unknown members. */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { checkTemplate } from '@/api/resources';
import { TemplateEditor } from '@/features/resources/templates/TemplateEditor';
import { SchemaForm } from '@/features/resources/templates/SchemaSettings';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { detailSchema } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@/api/resources', async (load) => ({ ...await load<object>(), checkTemplate: vi.fn() }));
vi.mock('@/ui/CodeView', () => ({ CodeView: ({ text, label, onChange, readOnly }: { text: string; label: string; onChange?: (text: string) => void; readOnly?: boolean }) => <textarea aria-label={label} value={text} readOnly={readOnly} onChange={(event) => onChange?.(event.target.value)} /> }));
const previousAppearance = useAppearance.getState(), wasDark = document.documentElement.classList.contains('dark'), previousTheme = document.documentElement.dataset.theme;
beforeEach(() => { vi.clearAllMocks(); vi.mocked(checkTemplate).mockResolvedValue({ valid: true, errors: [] }); });
afterEach(async () => { cleanup(); useAppearance.setState(previousAppearance); document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme; await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  // Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
  it(`offers all sections without a long form, and preserves edits and extension members between pages (${locale}/${theme})`, { timeout: 30_000 }, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), changed = vi.fn();
    function Form() { const [value, setValue] = useState<Record<string, unknown>>({ image: 'research:stable', extension: { keep: true } }); return <SchemaForm compact schema={detailSchema} spec={value} driver="sample" onChange={(next) => { setValue(next); changed(next); }} />; }
    show(<Form />);
    expect(screen.getByRole('textbox', { name: 'Image reference' })).toHaveValue('research:stable');
    expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument();
    await user.click(screen.getByRole('link', { name: i18n.t('templates.editor.sections.uses') }));
    expect(screen.getAllByRole('spinbutton')).toHaveLength(12);
    fireEvent.change(screen.getByRole('spinbutton', { name: 'Worker limit 1' }), { target: { value: '7' } });
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
    expect(screen.getByRole('spinbutton', { name: 'Worker limit 13' })).toBeVisible(); expect(screen.queryByRole('spinbutton', { name: 'Worker limit 1' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.previous') }));
    expect(screen.getByRole('spinbutton', { name: 'Worker limit 1' })).toHaveValue(7);
    expect(changed).toHaveBeenLastCalledWith({ image: 'research:stable', resources: { field_0: 7 }, extension: { keep: true } });
    const search = screen.getByRole('searchbox', { name: i18n.t('templateSettings.search') });
    await user.click(search); await user.paste('Worker limit 27'); expect(screen.getAllByRole('spinbutton')).toHaveLength(1); expect(screen.getByRole('spinbutton', { name: 'Worker limit 27' })).toBeVisible();
  });
}

it('keeps full JSON behind Details, extensions behind their explicit editor, and an invalid draft in place', async () => {
  const user = userEvent.setup(), valid = vi.fn();
  function Form() { const [text, setText] = useState(JSON.stringify({ image: 'research:stable', extension: { keep: true } })); return <TemplateEditor compact schema={detailSchema} driver="sample" kind="environment" text={text} onChange={setText} onValidityChange={valid} />; }
  show(<Form />);
  expect(screen.queryByRole('textbox', { name: i18n.t('templates.spec.json') })).not.toBeInTheDocument();
  expect(screen.queryByRole('switch', { name: i18n.t('templates.modeChoices.json.name') })).not.toBeInTheDocument();
  const editName = i18n.t('settings.compact.editValue', { name: i18n.t('templates.editor.other') });
  await user.click(screen.getByRole('button', { name: editName }));
  const group = screen.getByRole('group', { name: editName });
  expect(within(group).queryByText(i18n.t('common.details'), { exact: true })).not.toBeInTheDocument();
  const other = await within(group).findByRole('textbox', { name: i18n.t('templates.editor.other') });
  await waitFor(() => expect(checkTemplate).toHaveBeenCalledTimes(1));
  fireEvent.change(other, { target: { value: '{' } });
  await waitFor(() => expect(valid).toHaveBeenLastCalledWith(false));
  expect(screen.getByRole('searchbox', { name: i18n.t('templateSettings.search') })).toBeDisabled();
  await user.click(screen.getByRole('link', { name: i18n.t('templates.editor.sections.uses') }));
  expect(screen.getByRole('textbox', { name: 'Image reference' })).toBeVisible(); expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument();
  fireEvent.change(other, { target: { value: '{"extension":{"keep":true}}' } });
  await waitFor(() => expect(valid).toHaveBeenLastCalledWith(true));
  await user.click(screen.getByRole('link', { name: i18n.t('templates.editor.sections.uses') })); expect(screen.getByRole('spinbutton', { name: 'Worker limit 1' })).toBeVisible();
});

it('opens the complete configuration through its technical disclosure, retaining the form values', async () => {
  const user = userEvent.setup();
  function Form() { const [text, setText] = useState(JSON.stringify({ image: 'research:stable', extension: { keep: true } })); return <TemplateEditor compact schema={detailSchema} driver="sample" kind="environment" text={text} onChange={setText} />; }
  show(<Form />);
  await user.click(screen.getByText(i18n.t('common.details'), { exact: true }));
  await user.click(await screen.findByRole('switch', { name: i18n.t('templates.modeChoices.json.name') }));
  const editor = await screen.findByRole('textbox', { name: i18n.t('templates.spec.json') });
  expect(editor).toHaveValue(JSON.stringify({ image: 'research:stable', extension: { keep: true } }));
  await user.click(screen.getByText(i18n.t('common.details'), { exact: true })); expect(editor).not.toBeVisible();
});
