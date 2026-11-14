/** Settings keep their own section column while the independent global navigation opens and closes. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import { type AnchorHTMLAttributes, type ReactNode, useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { adminModeKey } from '@/api/adminMode';
import { SettingsFrame } from '@/features/settings/Frame';
import { managementSections, personalSections, personalTitle, settingsNavigation, type SettingsNavigation } from '@/features/settings/navigation';
import type { AdminMode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { Section } from '@/shell/Page';
import { SectionContent, SectionNav } from '@/ui/SectionNav';
import { show } from './support';

vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  Link: ({ to, params, children, activeOptions: _activeOptions, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; params?: Record<string, string>; children?: ReactNode; activeOptions?: unknown }) => {
    return <a {...props} href={to.replace(/\$(\w+)/g, (_match: string, key: string) => params?.[key] ?? '')}>{children}</a>;
  },
}));
const appearance = useAppearance.getState();
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const caches = new Set<QueryClient>();
afterEach(async () => {
  cleanup();
  for (const cache of caches) cache.clear();
  caches.clear(); useAppearance.setState(appearance); await setLocale('en');
});

function Frame({ scope }: { scope: SettingsNavigation }) {
  const [sidebar, setSidebar] = useState(true);
  const title = scope.kind === 'personal' ? personalTitle('profile', i18n.t) : i18n.t('admin.sections.overview.title');
  const label = i18n.t(scope.kind === 'personal' ? 'settings.personal.nav' : 'admin.nav.label');
  return <>
    <button onClick={() => setSidebar(!sidebar)}>Toggle sidebar</button>
    {sidebar && <nav aria-label="Project navigation"><a href="/org/acme">Acme</a></nav>}
    <SettingsFrame title={title} lead="One explanation" label={label} current={scope.current}
      items={scope.kind === 'personal' ? personalSections(i18n.t) : managementSections(i18n.t)}>
      <Section id="scope-body" title={title} lead="One explanation"><p>Setting controls</p></Section>
    </SettingsFrame>
  </>;
}

function draw(scope: SettingsNavigation) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  caches.add(cache);
  // The native sidebar consumes the shell's cached session answer without starting its own read.
  cache.setQueryData<AdminMode>(adminModeKey, { available: true, as_admin: true });
  return show(<QueryClientProvider client={cache}><Frame scope={scope} /></QueryClientProvider>);
}

for (const [locale, theme] of modes) it(`keeps one title and one navigation when the sidebar is shown, closed and restored in ${locale}/${theme}`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  for (const scope of [{ kind: 'personal', current: 'profile' }, { kind: 'system', current: 'overview' }] as const) {
    const user = userEvent.setup(); const view = draw(scope);
    const label = i18n.t(scope.kind === 'personal' ? 'settings.personal.nav' : 'admin.nav.label');
    expect(screen.getAllByRole('navigation', { name: label })).toHaveLength(1);
    expect(screen.getAllByRole('heading')).toHaveLength(1);
    expect(screen.getAllByText('One explanation')).toHaveLength(1);
    expect(view.container.querySelector('[data-settings-frame] [data-section-nav]')).not.toBeNull();
    expect(view.container.querySelector('#scope-body')).toHaveAccessibleName(screen.getByRole('heading', { level: 1 }).textContent);
    await user.click(screen.getByRole('button', { name: 'Toggle sidebar' }));
    expect(screen.queryByRole('navigation', { name: 'Project navigation' })).not.toBeInTheDocument();
    const nav = screen.getByRole('navigation', { name: label });
    expect(within(nav).queryByRole('combobox')).not.toBeInTheDocument();
    expect(nav.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: 'Toggle sidebar' }));
    expect(screen.getAllByRole('navigation', { name: label })).toHaveLength(1);
    expect(view.container.querySelector('[data-settings-frame] [data-section-nav]')).not.toBeNull();
    view.unmount();
  }
});

const sections = ['general', 'defaults', 'audit'];
const items = (chosen: (href: string | null) => void) => sections.map((id) => ({ id, label: id,
  link: <a href={`/topic/place?tab=settings&section=${id}`} onClick={(event) => { event.preventDefault(); chosen(event.currentTarget.getAttribute('href')); }} />,
}));

it('activates the real destination of a pressed section name, retaining its scope and section', async () => {
  const chosen = vi.fn(); const user = userEvent.setup();
  show(<SectionNav label="Sections" current="general" items={items(chosen)} />);
  await user.click(screen.getByRole('link', { name: 'audit' }));
  expect(chosen).toHaveBeenCalledExactlyOnceWith('/topic/place?tab=settings&section=audit');
  expect(screen.queryByRole('button', { name: /More|更多/ })).not.toBeInTheDocument();
});

it('is never a dropdown at 400 px: the names are one row that scrolls sideways, and a list from 600 px of room', () => {
  const width = window.innerWidth;
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 400 });
  window.dispatchEvent(new Event('resize'));
  try {
    show(<SectionNav label="Sections" current="defaults" items={items(vi.fn())} />);
    const nav = screen.getByRole('navigation', { name: 'Sections' });
    expect(within(nav).queryByRole('combobox')).not.toBeInTheDocument();
    expect(within(nav).queryByRole('listbox')).not.toBeInTheDocument();
    expect(within(nav).getAllByRole('link')).toHaveLength(sections.length);
    expect(within(nav).getByRole('link', { name: 'defaults' })).toHaveAttribute('aria-current', 'page');
    const row = nav.querySelector('ul');
    expect(row).toHaveClass('flex-row', 'overflow-x-auto');
    expect(row?.className).toContain('@min-[600px]:flex-col');
    expect(nav.className).toContain('@min-[600px]:sticky');
    // A list drawn inside the content of another is the row at every width, so that two lists never share a narrow column.
    expect(within(nav).getAllByRole('listitem')).toHaveLength(sections.length);
  } finally { Object.defineProperty(window, 'innerWidth', { configurable: true, value: width }); }
});

it('draws a list inside the content of another list as a strip whatever the room', () => {
  show(<SectionContent><SectionNav label="Groups" current="defaults" items={items(vi.fn())} /></SectionContent>);
  const nav = screen.getByRole('navigation', { name: 'Groups' });
  expect(nav).toHaveAttribute('data-section-strip');
  expect(nav.querySelector('ul')?.className).not.toContain('@min-[600px]:flex-col');
});

it('asks once before a section name leads away from unsaved work, and leaves when it is confirmed', async () => {
  const chosen = vi.fn(); const user = userEvent.setup();
  show(<SectionNav label="Sections" current="general" unsaved items={items(chosen)} />);
  await user.click(screen.getByRole('link', { name: 'audit' }));
  expect(chosen).not.toHaveBeenCalled();
  expect(await screen.findByRole('alertdialog')).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('settingsEditors.leave.confirm') }));
  expect(chosen).toHaveBeenCalledExactlyOnceWith('/topic/place?tab=settings&section=audit');
  // Pressing where the page already is asks nothing.
  await user.click(screen.getByRole('link', { name: 'general' }));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
});

it('uses the section sidebar for every settings scope while leaving ordinary content navigation intact', () => {
  expect(settingsNavigation('/settings/interface')).toEqual({ kind: 'personal', current: 'interface' });
  expect(settingsNavigation('/admin/settings')).toEqual({ kind: 'system', current: 'settings' });
  expect(settingsNavigation('/org/acme/settings/general')).toEqual({ kind: 'org', address: 'acme', current: 'general' });
  expect(settingsNavigation('/org/acme.dep2/settings/members')).toEqual({ kind: 'org', address: 'acme.dep2', current: 'members' });
  expect(settingsNavigation('/project/web/settings')).toEqual({ kind: 'project', id: 'web', current: 'general' });
  expect(settingsNavigation('/project/web/settings', { section: 'defaults' })).toEqual({ kind: 'project', id: 'web', current: 'defaults' });
  expect(settingsNavigation('/topic/login', { tab: 'settings', section: 'audit' })).toEqual({ kind: 'topic', id: 'login', current: 'audit' });
  expect(settingsNavigation('/topic/login', { tab: 'settings' })).toEqual({ kind: 'topic', id: 'login', current: 'general' });
  expect(settingsNavigation('/org/acme')).toBeNull();
  expect(settingsNavigation('/project/web')).toBeNull();
  expect(settingsNavigation('/topic/login')).toBeNull();
  expect(settingsNavigation('/topic/login', { tab: 'overview' })).toBeNull();
  expect(settingsNavigation('/usage')).toBeNull();
});
