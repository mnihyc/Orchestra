/** Resource choices show names and reasons, while their exact ids and copied locators keep their original meaning. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import type * as Personal from '@/api/personal';
import { afterEach, expect, it, vi } from 'vitest';
import { Card } from '@/features/finder/Card';
import { Where } from '@/features/start/Where';
import { TopicEnvironments } from '@/features/topic/Environments';
import type { NameHit } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { environment, options } from '../unit/start-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  Link: ({ to, params = {}, children, className }: { to: string; params?: Record<string, string>; children?: ReactNode; className?: string }) => <a className={className} href={to.replace(/\$(\w+)/g, (_match: string, key: string) => encodeURIComponent(params[key] ?? ''))}>{children}</a>,
}));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(), recordItemVisit: () => Promise.resolve() }));

const appearance = useAppearance.getState();
const themeBefore = document.documentElement.getAttribute('data-theme');
afterEach(async () => {
  cleanup(); useAppearance.setState(appearance); await setLocale('en'); vi.restoreAllMocks();
  if (themeBefore === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', themeBefore);
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`names an environment in its topic without repeating its routing address in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const workspace = { ...environment('workspace').environment, name: 'Alice workspace', address: 'acme.ada.acme-workspace' };
    const view = show(<TopicEnvironments topic={{ environments: [workspace], default_environment_id: workspace.id }} />);
    expect(screen.getByRole('link', { name: workspace.name })).toHaveAttribute('href', '/resources/environments/workspace');
    expect(screen.getByText(i18n.t('places.environments.default'))).toBeVisible();
    expect(view.container).not.toHaveTextContent(workspace.address);
  });

  it(`keeps names plain and an unavailable environment's reason readable without choosing it in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const available = environment('ready', { isDefault: true });
    const blocked = { ...environment('blocked', { usable: false }), reason: null };
    const choose = vi.fn<(id: string) => void>();
    show(<Where compact place={{ id: 'topic', name: 'Release', kind: 'topic', parentId: 'project', archived: false, status: 'open', folder: false }}
      homeId="acme" machines={[]} options={options({ environments: [available, blocked], can_create: { allowed: false, missing: [] } })} value={blocked.environment.id} onChange={choose} />);
    const trigger = screen.getByRole('combobox', { name: i18n.t('start.where.title') });
    expect(trigger).toHaveTextContent(blocked.environment.name);
    await userEvent.click(trigger);
    const list = await screen.findByRole('listbox');
    const off = within(list).getByRole('option', { name: blocked.environment.name });
    expect(off).toHaveAttribute('aria-disabled', 'true');
    expect(off).toHaveAccessibleDescription();
    expect(within(off).getByText(i18n.t('start.where.closed'))).toBeVisible();
    expect(within(list).getByText(i18n.t('start.where.topicDefault'))).toBeVisible();
    expect(list).not.toHaveTextContent(available.environment.address);
    expect(list).not.toHaveTextContent(blocked.environment.address);
    await userEvent.click(off);
    expect(choose).not.toHaveBeenCalled();
    await userEvent.click(within(list).getByRole('option', { name: available.environment.name }));
    expect(choose).toHaveBeenCalledExactlyOnceWith(available.environment.id);
  });

  it(`copies a group's exact locator without printing it as a second identity in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    const hit: NameHit = { id: 'crew', kind: 'group', name: 'Release crew', address: 'acme.crew', username: null, avatar: null, path: [], owner: null,
      href: null, tag: null, thread_attention: null, color: null, custom_color: null, archived: false, recent: true };
    const view = show(<Card hit={hit} onBack={() => undefined} onPage={null} />);
    expect(screen.getByRole('heading', { name: hit.name })).toBeVisible();
    expect(view.container).not.toHaveTextContent('acme.crew');
    const copy = screen.getByRole('button', { name: i18n.t('finder.card.copyAddress') });
    await user.click(copy);
    await waitFor(() => expect(write).toHaveBeenCalledExactlyOnceWith('acme.crew'));
    await waitFor(() => expect(within(copy).getByRole('status')).toHaveTextContent(i18n.t('common.copied')));
    expect(copy).toHaveAccessibleName(i18n.t('finder.card.copyAddress'));
    expect(copy).toBeVisible();
  });
}
