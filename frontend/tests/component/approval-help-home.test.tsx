/** Approval bodies retain their facts and decisions while all guide entrances belong outside their cards. */
import { screen, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import type * as TreeStore from '@/realtime/stores/tree';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CapabilityCard, type CapabilityApproval } from '@/features/approvals/Capability';
import { ApprovalCard } from '@/features/timeline/items/Approval';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { PortRequestCard } from '@/features/ports/Request';
import type { PortRequest } from '@/features/ports/presentation';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ALLOW_DENY, approvalItem, harnessApproval } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), Link: ({ children }: { children: ReactNode }) => <a href="#thread">{children}</a> }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: { display_name: string } }) => <span>{user.display_name}</span> }));
vi.mock('@/realtime/stores/tree', async (load) => ({ ...await load<typeof TreeStore>(), useNodePath: () => ({ path: [] }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const owner = userRef(me(1, 'ada', 'Ada'));
const context: TimelineContext = { threadId: 'help-thread', threadName: 'Deploy', detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
const native = harnessApproval('command-help', { threadId: context.threadId, itemId: 'command-item', title: 'Run checks', requester: owner,
  request: { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'check redirect', cwd: '/workspace', reason: null } } });
const capability: CapabilityApproval = { ...native, kind: 'capability', id: 'capability-help', title: 'Ports through a link',
  request: { capability: 'port_link', label: 'Ports through a link', risk: 'high', reason: 'A webhook must reach this service.',
    environment: { id: 'environment-1', name: 'Release workspace', owner, address: 'internal.release.workspace', placement: 'managed', ports_exposed: false, status: 'running' },
    host: { id: 'host-1', name: 'Release host', address: 'internal.release.host' }, consequences: ['Other people may connect to this port.'] } };
const port: PortRequest = { id: 'port-help', revision: 1, environment_id: 'environment-1', port: 3000, way: 'link', reason: 'A webhook must call this service.',
  agent: 'Deploy', address: 'https://port.example', audience: 'Anyone with the reviewed link.', until: null, can_decide: true, state: 'pending', transports: 'tcp_udp', link_carriers: 'both' };
let locale = currentLocale();
let appearance = useAppearance.getState();
let theme: string | null = null;
beforeEach(() => { locale = currentLocale(); appearance = useAppearance.getState(); theme = document.documentElement.getAttribute('data-theme'); });
afterEach(async () => { useAppearance.setState(appearance); await setLocale(locale); if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('approval help in %s / %s', (language, look) => {
  beforeEach(async () => { await setLocale(language); useAppearance.setState({ locale: language, theme: look, resolvedTheme: look }); document.documentElement.setAttribute('data-theme', look); });
  it('has no guide inside harness, capability or port cards while retaining the request and its acts', () => {
    const base = approvalItem('command-item', native.id);
    if (base.kind !== 'approval') throw new Error('The fixture is a request');
    const display = show(<><TimelineContextValue.Provider value={context}><div data-testid="harness"><ApprovalCard item={{ ...base, payload: { ...base.payload, approval: native } }} /></div></TimelineContextValue.Provider>
      <div data-testid="capability"><CapabilityCard approval={capability} live onSettled={vi.fn()} /></div>
      <div data-testid="port"><PortRequestCard request={port} /></div></>);
    for (const id of ['harness', 'capability', 'port']) {
      const card = screen.getByTestId(id);
      expect(card.querySelector('[data-help-guide], [data-help-article], [data-action="help"]')).toBeNull();
      for (const key of ['helpRelease.guides.approvals', 'helpRelease.guides.ports', 'helpRelease.guides.liveEnvironment'] as const) expect(within(card).queryByText(i18n.t(key))).not.toBeInTheDocument();
    }
    expect(within(screen.getByTestId('harness')).getByText('check redirect')).toBeVisible();
    const cap = screen.getByTestId('capability');
    expect(within(cap).getAllByText('Release workspace')).toHaveLength(1);
    expect(within(cap).getByText('Release host')).toBeVisible();
    expect(cap).not.toHaveTextContent('internal.release');
    expect(within(cap).getByText('A webhook must reach this service.')).toBeVisible();
    const decisions = within(cap).getAllByRole('button', { name: new RegExp(i18n.t('capabilityWords.port_link.name')) });
    expect(decisions).toHaveLength(2);
    for (const decision of decisions) expect(decision).toBeEnabled();
    expect(within(screen.getByTestId('port')).getByText('A webhook must call this service.')).toBeVisible();
    display.rerender(<PortRequestCard request={{ ...port, state: 'allowed', can_decide: false }} />);
    const settled = screen.getByRole('region', { name: i18n.t('ports.requestTitle') });
    expect(settled).toHaveTextContent(i18n.t('ports.requestState.allowed'));
    expect(settled).toHaveClass('border-border');
    expect(settled).not.toHaveClass('border-warning');
    expect(settled.querySelector('[data-help-guide], [data-help-article], [data-action="help"]')).toBeNull();
  });
});
