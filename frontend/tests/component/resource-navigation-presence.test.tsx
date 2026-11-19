/** The visible native Resources view owns navigation, and its + opens existing creation flows. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Activity, useState, type AnchorHTMLAttributes, type ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { ResourcesView } from '@/features/navigation/Resources';
import { ResourceAddMenu } from '@/features/resources/ResourceCreation';
import { KINDS, ResourceNav } from '@/features/resources/ResourceNav';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { viewOf } from '@/features/workbench/views';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ to, activeProps: _activeProps, children, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; activeProps?: unknown; children?: ReactNode }) => {
    return <a href={to} {...props}>{children}</a>;
  },
  useNavigate: () => vi.fn(), useRouterState: () => '/resources/providers', useLocation: () => ({ pathname: '/resources/providers' }),
}));
vi.mock('@/features/resources/summary', () => ({ useResourceSummary: () => ({ data: { providers: 0, agents: 0, machines: 0, environments: 0, templates: 0, hosts: 0, mcp: 0, mounts: 0, skills: 0 }, isError: false }) }));
vi.mock('@/api/everything', () => ({ everythingQuery: () => ({ queryKey: ['unopened-resource-names'], initialPageParam: null, queryFn: () => Promise.resolve({ items: [], next_cursor: null }), getNextPageParam: () => undefined }) }));

// Only the dialogs are substituted; the real shared menu, lazy dispatch and lifetime are exercised.
vi.mock('@/features/resources/providers/AddProvider', () => ({ AddProviderDialog: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="providers"><button onClick={onClose}>Close</button></div> }));
vi.mock('@/features/resources/agents/AddAgent', () => ({ AddAgentDialog: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="agents"><button onClick={onClose}>Close</button></div> }));
vi.mock('@/features/resources/machines/Enrollment', () => ({ AddMachineDialog: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="machines"><button onClick={onClose}>Close</button></div> }));
vi.mock('@/features/resources/hosts/AddHost', () => ({ AddHostDialog: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="hosts"><button onClick={onClose}>Close</button></div> }));
vi.mock('@/features/resources/templates/NewTemplate', () => ({ NewTemplateDialog: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="templates"><button onClick={onClose}>Close</button></div> }));
vi.mock('@/features/resources/environments/MakeEnvironment', () => ({ MakeEnvironmentDialog: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="environments"><button onClick={onClose}>Close</button></div> }));
vi.mock('@/features/resources/mcp/Edit', () => ({ EditMcp: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="mcp"><button onClick={onClose}>Close</button></div> }));
vi.mock('@/features/resources/mounts/Edit', () => ({ EditMount: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="mounts"><button onClick={onClose}>Close</button></div> }));
vi.mock('@/features/resources/skills/SkillDialog', () => ({ AddSkill: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="skills"><button onClick={onClose}>Close</button></div> }));

const appearance = useAppearance.getState();
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

function Navigation() {
  const [shown, setShown] = useState(true);
  return <>
    <button onClick={() => setShown(!shown)}>Toggle Resources view</button>
    <ViewOpeningContext.Provider value={shown ? 'resources-opening' : null}>
      <Activity mode={shown ? 'visible' : 'hidden'}><ResourcesView /></Activity>
    </ViewOpeningContext.Provider>
    <ResourceNav />
  </>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`has one set of kind links when navigation is shown, hidden and restored in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const user = userEvent.setup(), view = show(<Navigation />);
    expect(view.container.querySelector('[data-resource-navigation]')).toBeNull();
    const sidebar = screen.getByRole('navigation', { name: i18n.t('navigation.resources.title') });
    expect(within(sidebar).getAllByRole('link')).toHaveLength(9);
    for (const { name } of KINDS) expect(screen.getAllByRole('link', { name: i18n.t(`screens.${name}.title`) })).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: 'Toggle Resources view' }));
    expect(screen.queryByRole('navigation', { name: i18n.t('navigation.resources.title') })).not.toBeInTheDocument();
    expect(view.container.querySelector('[data-resource-navigation]')).not.toBeNull();
    for (const { name } of KINDS) expect(screen.getAllByRole('link', { name: i18n.t(`screens.${name}.title`) })).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: 'Toggle Resources view' }));
    expect(view.container.querySelector('[data-resource-navigation]')).toBeNull();
    expect(screen.getAllByRole('navigation', { name: i18n.t('navigation.resources.title') })).toHaveLength(1);
  });

  it(`uses Resources + for each native creation flow instead of adding a view in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    expect(viewOf('resource-navigation')?.add).toBe(ResourceAddMenu);
    const user = userEvent.setup(); show(<ResourceAddMenu />);
    for (const { name } of KINDS) {
      await user.click(screen.getByRole('button', { name: i18n.t('resources.controls.add') }));
      const menu = screen.getByRole('menu'); expect(within(menu).getAllByRole('menuitem')).toHaveLength(9);
      // The click starts a real lazy import. Await its resolution inside the same React update.
      await act(async () => {
        await user.click(within(menu).getByRole('menuitem', { name: i18n.t(`screens.${name}.title`) }));
        await vi.dynamicImportSettled();
      });
      const dialog = await screen.findByRole('dialog', { name });
      expect(screen.getAllByRole('dialog')).toHaveLength(1);
      await user.click(within(dialog).getByRole('button', { name: 'Close' }));
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    }
  });
}
