/** Screens-review regressions: an empty newcomer can finish a lesson, actual owned choices navigate, and Help stays readable. */
import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { client } from '@/api/client';
import { HelpContent } from '@/features/help/HelpView';
import { useHelpState } from '@/features/help/state';
import { TutorialHost, TutorialList } from '@/features/help/Tutorial';
import { TutorialPlaces, tutorialPlaces, type TutorialEnvironment } from '@/features/help/TutorialPlaces';
import { createTutorialDock, leaveTutorial, useTutorialRun } from '@/features/help/tutorialState';
import { EnvironmentNetwork } from '@/features/resources/environments/Network';
import type { ComponentProps } from 'react';
import { setLocale } from '@/i18n';
import { show } from './support';

const fake = vi.hoisted(() => ({
  path: '/help', rows: [] as unknown[], push: vi.fn(), open: vi.fn(), change: vi.fn(), overlay: vi.fn(), finish: vi.fn(), keep: vi.fn(),
}));
vi.mock('@tanstack/react-router', async (original) => ({
  ...await original<object>(),
  useRouter: () => ({ state: { location: { pathname: fake.path } }, history: { push: fake.push } }),
  useLocation: () => ({ pathname: fake.path, search: {}, searchStr: '', hash: '' }),
  useNavigate: () => vi.fn(),
  useMatches: () => fake.path,
  useRouterState: () => undefined,
}));
vi.mock('@/api/environments', () => ({ environmentsQuery: () => ({ queryKey: ['tutorial-places-test'], queryFn: () => Promise.resolve(fake.rows) }) }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: fake.open }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => fake.open }));
vi.mock('@/features/help/Dismissed', () => ({ useDismissed: () => ({ isFinished: () => false, stepOf: () => 0, keepStep: fake.keep, finish: fake.finish, reopenAll: () => undefined }) }));
vi.mock('@/features/workbench/Narrow', () => ({ useShape: () => 'wide' }));
vi.mock('@/features/workbench/store', () => ({ useWorkbench: (select: (state: { change: typeof fake.change; setOverlay: typeof fake.overlay }) => unknown) => select({ change: fake.change, setOverlay: fake.overlay }) }));
vi.mock('@/lib/viewChoices', () => ({ useViewChoice: () => ['screen', () => undefined] }));
vi.mock('@/help/localized', () => ({ useLocalizedGuide: () => ({ articles: [{ id: 'test-guide', title: 'Test guide', headings: [], screens: ['/help'], keywords: [], body: '# Test guide\n\nLast paragraph.' }] }) }));

function environment(id: string, owner: string): TutorialEnvironment {
  const person = { id: owner, username: owner, display_name: owner, kind: 'human' as const, avatar: { text: 'R', color: 0 } };
  return {
    id, name: `Environment ${id}`, address: `team.${owner}.${id}`, owner: person,
    access: { permissions: owner === 'reader' ? ['access.own'] : ['environment.view'], administers: false, admin_only: [], participating: true, through: [] },
    threads: [{ thread: { id: `thread-${id}`, kind: 'thread', name: `Thread ${id}`, redacted: false }, topic: { id: 'topic', kind: 'topic', name: 'Actual topic', redacted: false }, directory: '/', owner: person }],
  };
}

beforeEach(async () => {
  await setLocale('en');
  fake.path = '/help'; fake.rows = [];
  vi.clearAllMocks();
  client.session.setState({ status: 'signed_in', userId: 'reader' });
  useTutorialRun.setState({ id: null, step: 0, owner: null, origin: null, opener: null, picked: null, dock: null });
  useHelpState.setState({ article: null, origin: null, query: '' });
});
afterEach(() => { act(() => { leaveTutorial(); client.session.setState({ status: 'signed_out', userId: null }); }); });

describe('QS-004 and QS-005: lessons for a new reader', () => {
  it.each([
    ['how-approvals-work', 'How approvals work', 3, 'approvals-walk'],
    ['let-agents-open-ports', 'Let agents open ports without asking', 4, 'ports-walk'],
    ['change-a-running-environment', 'Change an environment while it runs', 3, 'live-environment-walk'],
    ['who-can-see-and-do-what', 'Who can see and do what', 3, 'access-walk'],
  ] as const)('can finish %s with no real requests, ports or places', async (article, title, count, id) => {
    show(<><TutorialList article={article} /><div ref={createTutorialDock()} data-testid="flow-dock" /><TutorialHost /></>);
    const row = screen.getByRole('heading', { name: title, level: 3 }).closest('li');
    if (row === null) throw new Error('the lesson is not listed');
    await userEvent.click(within(row).getByRole('button', { name: 'Start tutorial' }));
    const player = screen.getByTestId('tutorial-player');
    expect(screen.getByTestId('flow-dock')).toContainElement(player);
    expect(player).not.toHaveClass('fixed');
    for (let step = 1; step < count; step += 1) {
      expect(within(player).getByText(`Step ${step} of ${count}`, { exact: true })).toBeVisible();
      expect(within(player).getByTestId('tutorial-example')).toBeVisible();
      const next = within(player).getByRole('button', { name: 'Continue with example' });
      expect(next).toBeEnabled();
      await userEvent.click(next);
    }
    await userEvent.click(within(player).getByRole('button', { name: 'Finish tutorial' }));
    expect(fake.finish).toHaveBeenCalledWith(id);
    expect(screen.queryByTestId('tutorial-player')).not.toBeInTheDocument();
    expect(fake.open).not.toHaveBeenCalled();
  });

  it('offers only the reader’s owned objects and opens the chosen real environment', async () => {
    const owned = environment('mine', 'reader');
    const other = environment('other', 'somebody-else');
    expect(tutorialPlaces([owned, other], 'reader', true).map((one) => one.id)).toEqual(['thread-mine']);
    fake.rows = [owned, other];
    show(<TutorialPlaces destination="environment-settings" />);
    const choose = await screen.findByRole('combobox', { name: 'Choose one of your environments' });
    await userEvent.click(choose);
    expect(screen.queryByRole('option', { name: /Environment other/ })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('option', { name: /Environment mine/ }));
    expect(fake.push).toHaveBeenCalledWith('/resources/environments/mine?tab=settings');
    expect(fake.open).not.toHaveBeenCalled();
  });

  it.each(['thread-parameters', 'thread-access'] as const)('opens the reader’s real thread for %s', async (destination) => {
    fake.rows = [environment('mine', 'reader'), environment('other', 'somebody-else')];
    show(<TutorialPlaces destination={destination} />);
    const choose = await screen.findByRole('combobox', { name: 'Choose one of your threads' });
    await userEvent.click(choose);
    expect(screen.queryByRole('option', { name: /Thread other/ })).not.toBeInTheDocument();
    const thread = screen.getByRole('option', { name: 'Thread mine' });
    expect(thread).toHaveAccessibleDescription('Choose this visible place.');
    expect(document.body).not.toHaveTextContent('team.reader.mine');
    await userEvent.click(thread);
    expect(fake.push).toHaveBeenCalledWith('/thread/thread-mine');
    if (destination === 'thread-parameters') {
      expect(fake.change).toHaveBeenCalledOnce();
      expect(fake.open).not.toHaveBeenCalled();
    } else {
      expect(fake.open).toHaveBeenCalledWith({ kind: 'thread', id: 'thread-mine', name: 'Thread mine' }, { share: false });
      expect(fake.change).not.toHaveBeenCalled();
    }
  });
});

describe('QS-014 and QS-024: meaningful guide links and Help flow', () => {
  it('hides the screen-about-Help tab on Help itself', () => {
    show(<HelpContent page />);
    expect(screen.queryByRole('tab', { name: /this screen/i })).not.toBeInTheDocument();
    expect(screen.getByRole('tab', { name: /^Articles/ })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('button', { name: /Test guide/ })).toBeVisible();
  });

  it('does not offer Read the guide from that guide’s own tutorial footer', () => {
    show(<TutorialList article="how-approvals-work" />);
    expect(screen.queryByRole('button', { name: 'Read the guide' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start tutorial' })).toBeVisible();
  });

  it('keeps the tutorial footer after the last paragraph inside the article’s scroll region', () => {
    fake.path = '/help/test-guide';
    show(<HelpContent page article="test-guide" />);
    const region = screen.getByRole('region', { name: 'Test guide' });
    const footer = screen.getByTestId('help-tutorials');
    expect(region).toContainElement(footer);
    expect(screen.getByText('Last paragraph.').compareDocumentPosition(footer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('restores the underlying Help dock after a second Help view closes', () => {
    const first = show(<div ref={createTutorialDock()} data-testid="first-dock" />);
    const second = show(<div ref={createTutorialDock()} data-testid="second-dock" />);
    expect(useTutorialRun.getState().dock).toBe(screen.getByTestId('second-dock'));
    second.unmount();
    expect(useTutorialRun.getState().dock).toBe(screen.getByTestId('first-dock'));
    first.unmount();
    expect(useTutorialRun.getState().dock).toBeNull();
  });

  it('has one section entrance on Network, with each distinct guide inside it', async () => {
    const network: ComponentProps<typeof EnvironmentNetwork>['environment'] = {
      id: 'mine', name: 'My environment', job: null, access: { permissions: ['environment.view'] },
      network: { mode: 'open', required_mode: 'open', network_mode: 'bridge', networks: [], proxy_url: null, allowed_hosts: [], sources: [], revision: 'r1', always: [], always_revision: 'a1', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null },
    };
    show(<EnvironmentNetwork environment={network} manage={false} />);
    expect(screen.queryByRole('button', { name: 'Let agents open ports without asking' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Help: Network' }));
    expect(screen.getAllByRole('button', { name: 'Let agents open ports without asking' })).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Let agents open ports without asking' })).toHaveTextContent('Let agents open ports without asking');
    expect(screen.getAllByRole('button', { name: 'Change an environment while it runs' })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: 'Change an environment while it runs' }));
    expect(fake.open).toHaveBeenCalledWith('change-a-running-environment');
    expect(screen.queryByRole('dialog', { name: 'Network' })).toBeNull();
  });
});
