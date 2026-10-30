/** The frame composes native actions once, omits impossible tab acts and offers compact, grouped view choices. */
import { DndContext } from '@dnd-kit/core';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { PanelsTopLeft } from 'lucide-react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import type * as Sources from '@/features/finder/sources';
import { Finder } from '@/features/finder/Finder';
import { commandEntries, commandRows } from '@/features/finder/local';
import { useCommands } from '@/features/finder/commands';
import type { Env, GroupId } from '@/features/finder/rows';
import { closeFinder } from '@/features/finder/store';
import { OpenViewCommands, showOpenViews } from '@/features/workbench/OpenViews';
import { SideBarView } from '@/features/workbench/Sections';
import { makeContainer, makeGroup, makeTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { tabEntries } from '@/features/workbench/Tabs';
import { registerView } from '@/features/workbench/views';
import { i18n, setLocale } from '@/i18n';
import { clearScreenHeading } from '@/shell/screen';
import { Menu } from '@/ui/Menu';
import { show } from './support';

vi.mock('@/features/navigation/area', () => ({ useAreaNavigation: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => () => undefined }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), orgsQuery: { queryKey: ['chrome-orgs'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/features/finder/sources', async (original) => ({ ...await original<typeof Sources>(),
  useEnv: (showCard: Env['showCard']): Env => ({ t: i18n.t, go: () => undefined, orgSlug: null, exists: () => true, showCard }),
  useAnswers: (input: Sources.AnswerInput): ReturnType<typeof Sources.useAnswers> => {
    const prefix = input.commandPrefix;
    const commands = useCommands().filter((command) => prefix === undefined || prefix === null || command.id.startsWith(prefix));
    return { answers: input.active ? { commands: commandRows(commandEntries(commands, input.env.t), input.query, input.env) } : {}, full: new Set<GroupId>(), partial: false, pending: false, failure: null };
  },
}));

const before = useWorkbench.getState();
const undo: (() => void)[] = [];
const duplicate = vi.fn(() => null);
const nativeAct = vi.fn();
beforeEach(() => {
  clearScreenHeading(); duplicate.mockClear(); nativeAct.mockClear();
  undo.push(registerView({ name: 'navigation', title: 'views.navigation.title', about: 'views.navigation.about', icon: PanelsTopLeft, screens: 'all', area: 'left', body: () => null, actions: duplicate,
    menu: ({ label, entries }) => <Menu label={label} trigger={<button>{label}</button>} entries={[{ id: 'native-copy', label: i18n.t('places.menu.copyLink'), onSelect: nativeAct }, ...entries]} /> }));
  undo.push(registerView({ name: 'resource-navigation', title: 'navigation.resources.title', about: 'navigation.resources.about', icon: PanelsTopLeft, screens: 'all', area: 'left', body: () => null }));
  undo.push(registerView({ name: 'resources', title: 'views.resources.title', about: 'views.resources.about', icon: PanelsTopLeft, screens: 'all', area: 'right', container: 'environment', body: () => null }));
  undo.push(registerView({ name: 'screen', title: 'views.screen.title', about: 'views.screen.about', icon: PanelsTopLeft, screens: 'all', area: 'main', closable: false, body: () => null }));
  useWorkbench.setState({ ...before, screen: 'thread', layout: { ...before.layout,
    main: makeGroup([makeTab('screen'), makeTab('history')]),
    left: { ...before.layout.left, open: true, active: 'navigation', containers: [makeContainer('navigation', [makeTab('navigation')]), makeContainer('resource-navigation', [makeTab('resource-navigation')])] },
    right: { ...before.layout.right, open: true, active: 'environment', containers: [makeContainer('environment', [makeTab('resources')])] },
  } });
});
afterEach(async () => { cleanup(); closeFinder(); for (const remove of undo.splice(0)) remove(); useWorkbench.setState(before); document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`puts native and layout actions behind one title menu at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const layout = useWorkbench.getState().layout;
    show(<DndContext><SideBarView bar="left" layout={layout.left} renderView={() => <p>Navigation body</p>} addView={<span />} /></DndContext>);
    const title = document.querySelector('[data-container-title]');
    if (!(title instanceof HTMLElement)) throw new Error('The bar has no title row');
    const menus = within(title).getAllByRole('button', { expanded: false });
    expect(menus).toHaveLength(1); expect(duplicate).not.toHaveBeenCalled();
    const user = userEvent.setup(); const trigger = menus[0];
    if (trigger === undefined) throw new Error('The combined menu has no trigger');
    await user.click(trigger);
    expect(screen.getByRole('menuitem', { name: i18n.t('places.menu.copyLink') })).toBeVisible();
    expect(screen.getByRole('menuitem', { name: i18n.t('workbench.section.hide') })).toBeVisible();
    expect(screen.getByRole('menuitemcheckbox', { name: i18n.t('views.navigation.title') })).toBeChecked();
    await user.click(screen.getByRole('menuitem', { name: i18n.t('places.menu.copyLink') }));
    expect(nativeAct).toHaveBeenCalledTimes(1);
  });

  it(`titles the actual chooser Views and exposes one name and group per row at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    showOpenViews(); show(<><OpenViewCommands /><Finder /></>);
    const chooser = await screen.findByRole('dialog', { name: i18n.t('workbench.openView.choose') });
    expect(within(chooser).getByRole('heading', { name: i18n.t('workbench.openView.choose') })).toBeVisible();
    const resourceLists = await within(chooser).findByRole('option', { name: i18n.t('workbench.choices.views.resource-navigation.name') });
    const resources = within(chooser).getByRole('option', { name: i18n.t('views.resources.title') });
    expect(within(chooser).getAllByRole('option', { name: i18n.t('views.resources.title') })).toHaveLength(1);
    expect(resourceLists.querySelector('[data-view-group]')).toHaveTextContent(i18n.t('navigation.resources.title'));
    expect(resources.querySelector('[data-view-group]')).toHaveTextContent(i18n.t('workbench.containers.environment.title'));
    expect(chooser.querySelector('[data-choice-description], [data-choice-layout], [cmdk-group-heading]')).toBeNull();
    const user = userEvent.setup(); const input = within(chooser).getByRole('combobox', { name: i18n.t('workbench.openView.search') });
    await user.type(input, i18n.t('workbench.choices.views.resource-navigation.name'));
    await within(chooser).findByRole('option', { name: i18n.t('workbench.choices.views.resource-navigation.name') });
    await user.keyboard('{Enter}');
    await waitFor(() => expect(useWorkbench.getState().layout.left.active).toBe('resource-navigation'));
    expect(useWorkbench.getState().layout.left.open).toBe(true);
  });
}

it('offers Show only for a background tab and permits Close for every content page', () => {
  let layout = useWorkbench.getState().layout;
  if (layout.main.kind !== 'group') throw new Error('Expected a single main group');
  const current = layout.main.tabs[0], background = layout.main.tabs[1];
  if (current === undefined || background === undefined) throw new Error('Both real tab choices are required');
  const change = (update: (value: typeof layout) => typeof layout) => { layout = update(layout); };
  expect(tabEntries(layout, current.id, i18n.t, change).map((entry) => entry.id)).not.toContain('show');
  expect(tabEntries(layout, current.id, i18n.t, change).map((entry) => entry.id)).toContain('close');
  const entries = tabEntries(layout, background.id, i18n.t, change);
  expect(entries.map((entry) => entry.id)).toContain('close');
  const reveal = entries.find((entry) => entry.id === 'show');
  if (reveal === undefined || !('onSelect' in reveal)) throw new Error('The background tab has no Show action');
  reveal.onSelect();
  expect(layout.main.active).toBe(background.id);
});
