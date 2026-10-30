/** Ports details prove recipient choices, ready carrier facts and bounded live caller disclosure. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';
import * as portApi from '@/api/ports';
import { PortEntries, OnceLink } from '@/features/ports/Entries';
import { PortRequestCard } from '@/features/ports/Request';
import { ProgramDialog } from '@/features/ports/Program';
import type { PortRequest } from '@/features/ports/presentation';
import type { PortApproval } from '@/features/ports/pending';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { portsEnvironment, portsProjection } from './ports-fixtures';
import { show } from './support';
import type { PortOpenReply } from '@/generated/api-types';

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="#">{children}</a> }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
function withQuery(body: ReactNode) { const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); return { ...show(<QueryClientProvider client={cache}>{body}</QueryClientProvider>), cache }; }
const request: PortRequest = { id: 'request-1', revision: 1, environment_id: 'environment-1', port: 3000, way: 'link', reason: 'A webhook must call this port.', agent: 'Main', address: 'https://port.example', audience: 'Anyone with this link can enter.', until: new Date(Date.now() + 3_600_000).toISOString(), can_decide: true, state: 'pending', transports: 'tcp_udp', link_carriers: 'both' };
describe('intentional audience and entry facts', () => {
  it('states thread disclosure, offers no capability grant without the server offer, and sends the narrower recipient explicitly', async () => {
    const decide = vi.spyOn(portApi, 'decidePortRequest').mockResolvedValue({ request: { ...request, state: 'allowed' }, opening: null, delivered_to: 'human', link_url_once: 'https://orch.example#one', browser_url_once: 'https://port.example?orch_token=one', helper_link_once: 'https://orch.example#one' });
    const { cache } = withQuery(<PortRequestCard request={request} onChanged={vi.fn()} />);
    expect(screen.getByText('This link is given to the agent and is readable in this thread.')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Grant to this environment' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('checkbox', { name: 'For me only' }));
    await userEvent.click(screen.getByRole('button', { name: 'Allow port 3000 once' }));
    expect(decide).toHaveBeenCalledWith(request, 'once', true);
    expect(await screen.findByRole('button', { name: 'Copy webhook URL' })).toBeVisible();
    expect(JSON.stringify(cache.getQueryCache().getAll())).not.toContain('orch_token'); cache.clear();
  });
  it('offers only proved helper transports and keeps the proof out of every command', () => {
    show(<><PortEntries entries={[{ method: 'helper', transports: ['tcp'], address: null, command: 'orch-connector port forward --token-stdin', needs: 'sign_in', identity_methods: ['peer_lookup'] }]} /><OnceLink delivery={{ url_once: 'https://orch.example#secret', browser_url_once: 'https://port.example?orch_token=secret', helper_link_once: 'https://orch.example#secret' }} transports={['udp']} /></>);
    expect(screen.getByText('orch-connector port forward --token-stdin --transport tcp')).toBeVisible();
    expect(screen.getByText('orch-connector port forward --transport udp --link-stdin')).toBeVisible();
    expect(screen.getByText('For a browser or webhook')).toBeVisible();
    expect(screen.getByText('Keep orch_token in the query when adding your service path. A POST needs no browser exchange.')).toBeVisible();
  });
  it('offers Grant for a link when the server names the environment approvers, and sends that capability decision', async () => {
    // This is the component world's typed producer, not evidence of a server admission.
    const world = new World(); const person = world.people.get('ada')!;
    world.ports.control('/set', { scene: 'full' });
    const opened = (await world.ports.handle(person, 'POST', `/threads/${ACME.repro}/port-requests`, new URLSearchParams(), {
      port: 3000, kind: 'link', reason: 'A text webhook', until: new Date(Date.now() + 3_600_000).toISOString(),
    }))?.body as PortOpenReply;
    const approval = (await world.ports.handle(person, 'GET', `/approvals/${opened.request?.id}`, new URLSearchParams(), {}))?.body as PortApproval;
    expect(approval.request.can_grant).toBe(true);
    const current = portApi.requestView(approval);
    const decide = vi.spyOn(portApi, 'decidePortRequest').mockResolvedValue({ request: { ...current, state: 'allowed' }, opening: null, delivered_to: 'thread', link_url_once: null });
    withQuery(<PortRequestCard request={current} onChanged={vi.fn()} />);
    expect(screen.getByText('This link is given to the agent and is readable in this thread.')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Grant to this environment' }));
    expect(decide).toHaveBeenCalledWith(current, 'grant', false);
  });
  it('repairs a target’s program using its current revision without calling the audience producer', async () => {
    const data = portsProjection(); const port = { ...data.ports[0]!, program: { scheme: 'http' as const, host_header: null, proxy_protocol: false }, revision: 0 };
    const save = vi.spyOn(portApi, 'updatePortProgram').mockResolvedValue({} as Awaited<ReturnType<typeof portApi.updatePortProgram>>);
    const opening = vi.spyOn(portApi, 'createOpening');
    withQuery(<ProgramDialog environmentId={portsEnvironment.id} port={port} onClose={vi.fn()} onChanged={vi.fn()} />);
    await userEvent.click(screen.getByText('How the program is spoken to'));
    await userEvent.click(screen.getByRole('radio', { name: 'HTTPS' }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Send a PROXY header' }));
    await userEvent.click(screen.getByRole('button', { name: 'Save program options' }));
    expect(save).toHaveBeenCalledWith(portsEnvironment.id, 3000, { http_scheme: 'https', host_header: null, proxy_protocol: true }, 0);
    expect(opening).not.toHaveBeenCalled();
  });
});
