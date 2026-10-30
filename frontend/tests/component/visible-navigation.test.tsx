/** Every primary choice stays on screen, including choices reached across wrapped lines. Geometry lives in the browser cases. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { FileText } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { expect, it, vi } from 'vitest';

import { i18n, setLocale } from '@/i18n';
import { FoldingStrip } from '@/ui/FoldingStrip';
import { Tabs } from '@/ui/Tabs';
import { ViewGroup } from '@/ui/ViewGroup';
import { ViewTab } from '@/ui/ViewTab';
import { show } from './support';

const VIEWS = ['outline', 'git', 'changes', 'files', 'environment', 'parameters'] as const;

for (const locale of ['en', 'zh-CN'] as const) for (const active of [false, true]) for (const compact of [false, true]) {
  it(`keeps a view's visible name beside its page heading (${locale}, active ${String(active)}, compact ${String(compact)})`, async () => {
    try {
      await setLocale(locale);
      const title = locale === 'en' ? 'Settings · Acme' : '设置 · Acme';
      const close = vi.fn();
      const user = userEvent.setup();
      show(<><h1>{title}</h1><ViewTab id="named-tab" panelId="named-panel" title={title} icon={<FileText aria-hidden />}
        active={active} tabbable compact={compact} menu={[]} onSelect={() => undefined} onClose={close} />
        <section id="named-panel" role="tabpanel" aria-labelledby="named-tab">Current page</section></>);
      const tab = screen.getByRole('tab', { name: title });
      expect(within(tab).getByText(title)).toBeVisible();
      expect(tab).toHaveTextContent(title);
      await user.click(tab);
      await user.keyboard('{Delete}');
      expect(close).toHaveBeenCalledExactlyOnceWith();
    } finally { await setLocale('en'); }
  });
}

function PageTabs() {
  const { t } = useTranslation();
  const [active, setActive] = useState<(typeof VIEWS)[number]>('outline');
  return <Tabs label={t('workbench.containers.label')} value={active} onValueChange={setActive} tabs={VIEWS.map((view) => ({
    value: view, label: t(`views.${view}.title`), disabled: view === 'git', panel: t(`views.${view}.about`),
  }))} />;
}

it.each(['en', 'zh-CN'] as const)('shows every page tab and skips only disabled choices by keyboard in %s', async (locale) => {
  try {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<PageTabs />);
    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(VIEWS.length);
    for (const tab of tabs) expect(tab).toBeVisible();
    expect(screen.queryByRole('button', { name: /More|更多|tabs out of view|未显示的标签/ })).not.toBeInTheDocument();
    const first = screen.getByRole('tab', { name: i18n.t('views.outline.title') });
    const last = screen.getByRole('tab', { name: i18n.t('views.parameters.title') });
    await user.click(last);
    expect(last).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tabpanel', { name: i18n.t('views.parameters.title') })).toHaveTextContent(i18n.t('views.parameters.about'));
    await user.keyboard('{Home}');
    await waitFor(() => expect(first).toHaveFocus());
    await user.keyboard('{ArrowRight}');
    await waitFor(() => expect(screen.getByRole('tab', { name: i18n.t('views.changes.title') })).toHaveFocus());
    expect(screen.getByRole('tab', { name: i18n.t('views.git.title') })).toBeDisabled();
    await user.keyboard('{End}');
    await waitFor(() => expect(last).toHaveFocus());
    expect(last).toHaveAttribute('aria-selected', 'true');
  } finally { await setLocale('en'); }
});

it('keeps every native link, focuses across lines and follows a link only when activated', async () => {
  const chosen = vi.fn<(id: string) => void>();
  const user = userEvent.setup();
  show(<FoldingStrip label="Resource pages" active="outline" items={VIEWS.map((view) => ({
    id: view, label: i18n.t(`views.${view}.title`), onSelect: () => chosen(view),
    content: <a href={`/resource/${view}`} onClick={(event) => { event.preventDefault(); chosen(view); }}>{i18n.t(`views.${view}.title`)}</a>,
  }))} />);
  const navigation = screen.getByRole('navigation', { name: 'Resource pages' });
  const links = within(navigation).getAllByRole('link');
  expect(links).toHaveLength(VIEWS.length);
  for (const [index, link] of links.entries()) {
    expect(link).toBeVisible();
    expect(link).toHaveAttribute('href', `/resource/${VIEWS[index]}`);
  }
  links[0]?.focus();
  await user.keyboard('{End}');
  expect(links.at(-1)).toHaveFocus();
  expect(chosen).not.toHaveBeenCalled();
  await user.keyboard('{Enter}');
  expect(chosen).toHaveBeenCalledExactlyOnceWith('parameters');
  await user.keyboard('{Home}');
  expect(links[0]).toHaveFocus();
});

function GroupTabs() {
  const [active, setActive] = useState('outline');
  const ids = [...VIEWS];
  const information = ids.map((id) => ({ id, title: i18n.t(`views.${id}.title`), description: i18n.t(`views.${id}.about`), closable: false, onSelect: () => setActive(id), onClose: () => undefined }));
  return <ViewGroup label="Main area" tabIds={ids} activeId={active} panelId="visible-group-panel" tabElementId={(id) => `visible-group-${id}`}
    tabInfo={information}
    renderTab={(id, state) => <ViewTab id={`visible-group-${id}`} panelId="visible-group-panel" title={information.find((tab) => tab.id === id)?.title ?? id} icon={<FileText aria-hidden />} {...state} closable={false} menu={[]} onSelect={() => setActive(id)} onClose={() => undefined} />}>
    {active}
  </ViewGroup>;
}

it.each(['en', 'zh-CN'] as const)('shows group tabs directly and keeps the all-tabs list compact in %s', async (locale) => {
  try {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<GroupTabs />);
    const strip = screen.getByRole('tablist', { name: 'Main area' });
    expect(within(strip).getAllByRole('tab')).toHaveLength(VIEWS.length);
    for (const tab of within(strip).getAllByRole('tab')) expect(tab).toBeVisible();
    const first = within(strip).getByRole('tab', { name: i18n.t('views.outline.title') });
    const last = within(strip).getByRole('tab', { name: i18n.t('views.parameters.title') });
    await user.click(last);
    expect(last).toHaveAttribute('aria-selected', 'true');
    await user.keyboard('{Home}');
    await waitFor(() => expect(first).toHaveFocus());
    expect(first).toHaveAttribute('aria-selected', 'true');
    const about = i18n.t('views.outline.about');
    expect(screen.queryByText(about)).not.toBeInTheDocument();
    const listName = i18n.t('workbench.allTabs', { name: 'Main area' });
    await user.click(screen.getByRole('button', { name: listName }));
    const menu = screen.getByRole('dialog', { name: listName });
    for (const view of VIEWS) {
      const item = within(menu).getByRole('option', { name: i18n.t(`views.${view}.title`) });
      expect(item).toBeVisible(); expect(item).not.toHaveAccessibleDescription();
      expect(within(menu).queryByText(i18n.t(`views.${view}.about`))).not.toBeInTheDocument();
      expect(within(menu).queryByRole('button', { name: i18n.t('workbench.tab.closeNamed', { title: i18n.t(`views.${view}.title`) }) })).not.toBeInTheDocument();
    }
    expect(within(menu).getAllByRole('option')).toHaveLength(VIEWS.length);
    await user.click(within(menu).getByRole('option', { name: i18n.t('views.parameters.title') }));
    expect(last).toHaveAttribute('aria-selected', 'true');
    await waitFor(() => expect(last).toHaveFocus());
    // Cancelling keeps normal menu restoration; keyboard selection hands focus to the tab.
    const listButton = screen.getByRole('button', { name: listName });
    await user.click(listButton);
    await user.keyboard('{Escape}');
    await waitFor(() => expect(listButton).toHaveFocus());
    await user.keyboard('{Enter}');
    const nextMenu = await screen.findByRole('dialog', { name: listName });
    const locator = within(nextMenu).getByRole('combobox', { name: i18n.t('workbench.workspace.findTab') });
    expect(locator).toHaveFocus();
    await user.type(locator, i18n.t('views.outline.title'));
    expect(within(nextMenu).getByRole('option', { name: i18n.t('views.outline.title') })).toBeVisible();
    await user.keyboard('{Enter}');
    await waitFor(() => expect(first).toHaveFocus());
    expect(first).toHaveAttribute('aria-selected', 'true');
  } finally { await setLocale('en'); }
});
