/** Detail navigation exposes every section, keeps drafts, and stops hidden readers without clipping content. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useEffect, useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { DetailPages, DetailSections, ResourceDetails, ResourceHeadingActs, ResourceTabs } from '@/features/resources/DetailSections';
import { Section } from '@/shell/Page';
import { Input } from '@/ui/Input';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

const originalAppearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
afterEach(async () => {
  cleanup(); useAppearance.setState(originalAppearance); document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps every section reachable by keyboard and pauses a hidden reader without losing its draft (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), started = vi.fn(), stopped = vi.fn();
    function Reading() { const [draft, setDraft] = useState(''); useEffect(() => { started(); return () => { stopped(); }; }, []); return <Input label={i18n.t('resources.edit.name')} value={draft} onChange={(event) => setDraft(event.target.value)} />; }
    const first = i18n.t('resourceDetails.layout.configuration'), second = i18n.t('resourceDetails.layout.software');
    show(<DetailSections label={i18n.t('resourceDetails.layout.sections')} sections={[{ value: 'first', label: first, panel: <Reading /> }, { value: 'second', label: second, panel: <p>{i18n.t('resourceDetails.layout.notReported')}</p> }]} />);
    await user.type(screen.getByRole('textbox', { name: i18n.t('resources.edit.name') }), 'Keep my draft');
    const choice = screen.getByRole('link', { name: second }); choice.focus(); await user.keyboard('{Enter}');
    await waitFor(() => expect(stopped).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: second })).toHaveAttribute('aria-current', 'page');
    await user.click(screen.getByRole('link', { name: first }));
    expect(screen.getByRole('textbox', { name: i18n.t('resources.edit.name') })).toHaveValue('Keep my draft');
    expect(started).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('button', { name: /^More$/i })).not.toBeInTheDocument();
  });

  it(`bounds a long settings list, retains edits between pages and blocks leaving an unfinished value (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark'); const user = userEvent.setup();
    function Field({ number }: { number: number }) { const [value, setValue] = useState(String(number)); return <Input label={`Field ${number}`} value={value} onChange={(event) => setValue(event.target.value)} />; }
    const items = Array.from({ length: 27 }, (_, index) => index + 1);
    const content = (blocked: boolean) => <DetailPages items={items} blocked={blocked}>{(page) => page.map((number) => <Field key={number} number={number} />)}</DetailPages>;
    const view = show(content(false));
    expect(screen.getAllByRole('textbox')).toHaveLength(12);
    await user.clear(screen.getByRole('textbox', { name: 'Field 1' })); await user.type(screen.getByRole('textbox', { name: 'Field 1' }), 'retained');
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
    expect(screen.getByRole('textbox', { name: 'Field 13' })).toBeVisible(); expect(screen.queryByRole('textbox', { name: 'Field 1' })).not.toBeInTheDocument();
    view.rerender(content(true)); expect(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.previous') })).toBeDisabled();
    view.rerender(content(false)); await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.previous') }));
    expect(screen.getByRole('textbox', { name: 'Field 1' })).toHaveValue('retained');
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') })); await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
    expect(screen.getAllByRole('textbox')).toHaveLength(3);
    expect(screen.getByRole('status')).toHaveTextContent(i18n.t('resourceDetails.layout.range', { from: 25, to: 27, count: 27 }));
    expect(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') })).toBeDisabled();
  });
}

it('loads technical content only after Details and keeps only one visible section title', async () => {
  const read = vi.fn(), user = userEvent.setup();
  function Technical() { useEffect(() => { read(); }, []); return <p>diagnostic-canary</p>; }
  const title = i18n.t('resourceDetails.overview');
  show(<ResourceTabs label={i18n.t('resourceDetails.layout.tabs')} value="overview" onValueChange={vi.fn()} tabs={[{ value: 'overview', label: title,
    panel: <Section id="about" title={title}><ResourceDetails><Technical /></ResourceDetails></Section> }]} />);
  expect(read).not.toHaveBeenCalled(); expect(screen.queryByText('diagnostic-canary')).not.toBeInTheDocument();
  expect(screen.getAllByRole('heading', { name: title })).toHaveLength(1);
  await user.click(screen.getByText(i18n.t('common.details'), { exact: true }));
  expect(await screen.findByText('diagnostic-canary')).toBeVisible(); expect(read).toHaveBeenCalledTimes(1);
});

it('has one entrance per act while retaining Delete and the native callback', async () => {
  const user = userEvent.setup(), edit = vi.fn(), start = vi.fn(), remove = vi.fn();
  show(<ResourceHeadingActs name="Research" primary={{ id: 'start', label: 'Start', icon: null, run: start }} edit={{ id: 'edit', label: 'Edit', icon: null, run: edit }}
    menu={[{ id: 'start', name: 'Start', onSelect: start }, { id: 'edit', name: 'Edit', onSelect: edit }, { id: 'rename', name: 'Rename', onSelect: edit }, { type: 'separator', id: 'gap' }, { id: 'delete', name: 'Delete', onSelect: remove }]} />);
  expect(screen.getAllByRole('button', { name: i18n.t('resources.act.editButton') })).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.menuOf', { name: 'Research' }) }));
  expect(screen.queryByRole('menuitem', { name: 'Start' })).not.toBeInTheDocument(); expect(screen.queryByRole('menuitem', { name: 'Edit' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('menuitem', { name: 'Delete' })); expect(remove).toHaveBeenCalledTimes(1); expect(start).not.toHaveBeenCalled(); expect(edit).not.toHaveBeenCalled();
});
