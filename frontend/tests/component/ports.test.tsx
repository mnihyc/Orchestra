/** The Ports states stay distinct and selecting or refreshing a listener grants no access. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { PortsSnapshot } from '@/features/ports/Ports';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { LetMoreIn } from '@/features/ports/OpenDialog';
import { PortConnections } from '@/features/ports/Connections';
import * as portApi from '@/api/ports';
import type { PortOpening, PortPublishPlan } from '@/features/ports/presentation';
import { opening as wireOpening, target as wireTarget } from '../doubles/ports-wire';
import { portCapability, portsEnvironment as environment, portsProjection } from './ports-fixtures';
import { show } from './support';
import { Markdown } from '@/ui/Markdown';
import { LocalAddressRewrite } from '@/lib/markdown';
import { portAddressFor } from '@/features/ports/model';
import type { PortOpeningPlan } from '@/generated/api-types';

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="#">{children}</a> }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/ports/Request', () => ({ PortRequests: () => null }));
vi.mock('@/api/search', () => ({ namesQuery: () => ({ queryKey: ['port-test-names'], queryFn: () => Promise.resolve([]) }) }));

const sessionBefore = client.session.getState();
beforeEach(() => { client.session.setState({ status: 'signed_in', userId: 'ports-reader' }); });
afterEach(() => { client.session.setState(sessionBefore); });

describe('Ports shows what the environment actually knows', () => {
  it('shows TCP and UDP, first seen and visible origin, while Internal has no outside action', async () => {
    const data = portsProjection('full');
    const refresh = vi.fn();
    show(<QueryClientProvider client={new QueryClient()}><PortsSnapshot environment={environment} data={data} onRefresh={refresh} /></QueryClientProvider>);
    await userEvent.click(screen.getAllByText('Listeners')[0]!);
    await userEvent.click(screen.getAllByText('Listeners')[1]!);
    expect(screen.getAllByText(/TCP 3000/).length).toBeGreaterThan(0);
    await waitFor(() => expect(screen.getAllByText('web-server', { exact: true })).toHaveLength(1));
    expect(screen.getByText('web-server',{exact:true})).toBeVisible();
    expect(screen.getByText(/UDP 5353/)).toBeVisible();
    expect(screen.getByText('Internal')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(refresh).toHaveBeenCalledOnce();
  });
  it('reports actual UDP drop reasons without claiming delivery or retaining traffic', () => {
    const data = portsProjection(); data.snapshot.udp_dropped = { oversized: 2, full: 3, senders: 0, local_send: 0, link_lost: 0 };
    show(<QueryClientProvider client={new QueryClient()}><PortsSnapshot environment={environment} data={data} onRefresh={vi.fn()} /></QueryClientProvider>);
    expect(screen.getByText('UDP datagrams dropped')).toBeVisible(); expect(screen.getByText('Oversized: 2')).toBeVisible(); expect(screen.getByText('Buffer full: 3')).toBeVisible();
  });
  it('labels offline observation old rather than claiming that nothing listens', () => {
    const data = portsProjection('offline');
    show(<QueryClientProvider client={new QueryClient()}><PortsSnapshot environment={environment} data={data} onRefresh={vi.fn()} /></QueryClientProvider>);
    expect(screen.getByText('Old observation')).toBeVisible();
    expect(screen.queryByText('Nothing is listening yet')).not.toBeInTheDocument();
  });
  it('shows unsupported discovery separately from a successful empty snapshot', () => {
    const data = portsProjection('unsupported');
    show(<QueryClientProvider client={new QueryClient()}><PortsSnapshot environment={environment} data={data} onRefresh={vi.fn()} /></QueryClientProvider>);
    expect(screen.getByText('This platform cannot list scoped listeners')).toBeVisible();
    expect(screen.queryByText('Nothing is listening yet')).not.toBeInTheDocument();
  });
  it('says where egress is open that a program can still publish itself, and says nothing where it is restricted', () => {
    const data = portsProjection();
    const view = show(<QueryClientProvider client={new QueryClient()}><PortsSnapshot environment={environment} data={data} onRefresh={vi.fn()} /></QueryClientProvider>);
    expect(screen.getByText(/through its own outbound tunnel/)).toBeVisible();
    view.unmount();
    const restricted = { ...environment, network: { ...environment.network, mode: 'restricted' } } as typeof environment;
    show(<QueryClientProvider client={new QueryClient()}><PortsSnapshot environment={restricted} data={data} onRefresh={vi.fn()} /></QueryClientProvider>);
    expect(screen.queryByText(/through its own outbound tunnel/)).not.toBeInTheDocument();
  });
  it('shows each audience as a capability: denied, asked, or granted with whose grant, and grants only what may be granted', async () => {
    const data = portsProjection(); data.capabilities = [portCapability('place', 'ask'), portCapability('link', 'ask', true), portCapability('host', 'deny')];
    const grant = vi.spyOn(portApi, 'grantPortCapability').mockResolvedValue(portCapability('place', 'ask', true));
    const refresh = vi.fn();
    show(<QueryClientProvider client={new QueryClient()}><PortsSnapshot environment={environment} data={data} onRefresh={refresh} /></QueryClientProvider>);
    const grants = screen.getByTestId('port-grants');
    expect(within(grants).getByTestId('port-grant-link')).toHaveTextContent('Granted to this environment by Tester');
    expect(within(grants).getByTestId('port-grant-host')).toHaveTextContent('Denied by the capability policy');
    expect(within(within(grants).getByTestId('port-grant-host')).queryByRole('button')).not.toBeInTheDocument();
    expect(within(within(grants).getByTestId('port-grant-link')).getByRole('button', { name: 'Take back' })).toBeVisible();
    await userEvent.click(within(within(grants).getByTestId('port-grant-place')).getByRole('button', { name: 'Grant to this environment' }));
    expect(grant).not.toHaveBeenCalled();
    const dialog = await screen.findByRole('dialog');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Grant to this environment' }));
    expect(grant).toHaveBeenCalledWith(environment.id, 'port_place');
    expect(refresh).toHaveBeenCalled();
    grant.mockRestore();
  });
  it('leads a local address in a message to the environment port through Orch, and leaves other addresses alone', () => {
    const rewrite = (href: string) => portAddressFor(href, [{ port: 3000, url: 'https://e1-p3000.ports.example' }], 'environment-1');
    show(<LocalAddressRewrite.Provider value={rewrite}><Markdown text={'See [the app](http://localhost:3000/login?next=1), http://localhost:9999/x and [docs](https://example.org/a).'} /></LocalAddressRewrite.Provider>);
    expect(screen.getByRole('link', { name: 'the app' })).toHaveAttribute('href', 'https://e1-p3000.ports.example/login?next=1');
    expect(screen.getByRole('link', { name: 'http://localhost:9999/x' })).toHaveAttribute('href', '/resources/environments/environment-1?tab=ports&port=9999');
    expect(screen.getByRole('link', { name: 'docs' })).toHaveAttribute('href', 'https://example.org/a');
  });
  it('shows a real empty state and the server’s sixteen-port policy', async () => {
    const data = portsProjection('empty');
    show(<QueryClientProvider client={new QueryClient()}><PortsSnapshot environment={environment} data={data} onRefresh={vi.fn()} /></QueryClientProvider>);
    expect(screen.getAllByText('Nothing is listening yet')[0]).toBeVisible();
    await userEvent.click(screen.getByText('Policies and limits'));
    expect(screen.getByText('16')).toBeVisible();
  });
});

const opening = (way: PortOpening['way']): PortOpening => ({ id: 'opening-1', environment_id: environment.id, port: 3000, way, transports: 'tcp_udp', opened_by: { id: 'user-1', username: 'tester', display_name: 'Tester', kind: 'human', avatar: { text: 'T', color: 0 } }, opened_at: new Date().toISOString(), until: null, desired: true, state: 'open', reason: null, name: 'stable-p3000', url: 'https://stable-p3000.ports.example', host: null, host_port: null, fixed_host_port: false, revision: 1, place: null, viewers: null, link_carriers: 'both', http: { scheme: 'http', host_header: null }, connection_count: 3, can: { enter: true, close: true, change: true } });

describe('a person deliberately widens a port', () => {
  it('requires expiry before creation and keeps a once-returned secret outside all query and mutation state', async () => {
    const data = portsProjection(); const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const create = vi.spyOn(portApi, 'createOpening').mockResolvedValue({ opening: wireOpening(opening('link')), request: null, link: { recipient: 'person', delivered_to: 'user-1', url_once: 'https://service.example#once-only', browser_url_once: 'https://service.example?orch_token=once-only', helper_link_once: 'https://service.example#once-only' } });
    const review: PortOpeningPlan = { preflight_id: 'link-plan', expires_at: new Date(Date.now() + 60_000).toISOString(), target: wireTarget(data.ports[0]!, environment.id), program_revision: 0, request: { port: 3000, kind: 'link', reason: '' }, audience: { place: null, count: null, sentence: 'Anyone with this link can use this port.' }, candidate: null, disclosure: null, blockers: [] };
    vi.spyOn(portApi, 'preflightOpening').mockResolvedValue(review);
    const panel = show(<QueryClientProvider client={cache}><LetMoreIn port={data.ports[0]!} environment={environment} data={data} onClose={vi.fn()} onChanged={vi.fn()} onError={vi.fn()} /></QueryClientProvider>);
    await userEvent.click(screen.getByRole('radio', { name: 'Link' }));
    expect(create).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Create link' })).toHaveAttribute('aria-disabled', 'true');
    const input = screen.getByLabelText('Expires at');
    const { fireEvent, waitFor } = await import('@testing-library/react');
    const localFuture = Date.now() + 3_600_000 - new Date().getTimezoneOffset() * 60_000;
    fireEvent.change(input, { target: { value: new Date(localFuture).toISOString().slice(0, 16) } });
    await userEvent.click(screen.getByRole('button', { name: 'Review address' }));
    await userEvent.click(screen.getByRole('button', { name: 'Create link' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Copy link' })).toBeVisible());
    expect(create).toHaveBeenCalledOnce();
    expect(JSON.stringify(cache.getQueryCache().getAll().map((query) => query.state.data))).not.toContain('once-only');
    expect(cache.getMutationCache().getAll()).toHaveLength(0);
    panel.unmount(); expect(screen.queryByText(/once-only/)).not.toBeInTheDocument();
    cache.clear();
  });
  it('shows a directory publisher without a Host page and uses its exact stored candidate', async () => {
    const data = portsProjection(); const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const plan: PortPublishPlan = { preflight_id: 'plan-1', expires_at: new Date(Date.now() + 60_000).toISOString(), environment_id: environment.id, port: 3000, transports: 'tcp_udp', host: null, publisher: { id: 'machine-1', name: 'Dedicated VM', online: true, platform_os: 'linux', placement: 'connected' }, host_ports_revision: 2, host_port: 41003, fixed: false, address: 'host.example:41003', audience: '', blockers: [] };
    vi.spyOn(portApi, 'previewPublication').mockResolvedValue(plan);
    const publish = vi.spyOn(portApi, 'createOpening').mockResolvedValue({ opening: wireOpening({ ...opening('host'), state: 'opening', host_port: 41003, url: null, host_allocated: false }), request: null, link: null });
    show(<QueryClientProvider client={cache}><LetMoreIn port={data.ports[0]!} environment={environment} data={data} onClose={vi.fn()} onChanged={vi.fn()} onError={vi.fn()} /></QueryClientProvider>);
    const hostAudience = screen.getByRole('radio', { name: 'Host' });
    expect(hostAudience).toHaveAccessibleName('Host');
    expect(screen.getByText('Publish a number on the host for the audience shown in the plan.')).toBeVisible();
    await userEvent.click(hostAudience);
    expect(screen.getByRole('radio', { name: 'TCP and UDP' })).toBeChecked();
    await userEvent.click(screen.getByRole('button', { name: 'Review address' }));
    expect(await screen.findByText('Anybody who reaches host.example:41003 reaches this. Orch does not ask them who they are.')).toBeVisible();
    expect(screen.getByText(/Dedicated VM/)).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Publish this address' }));
    expect(publish).toHaveBeenCalledWith(environment.id, 3000, expect.any(String), 'plan-1');
    expect(screen.queryByRole('button', { name: 'Copy address' })).not.toBeInTheDocument();
    cache.clear();
  });
  it('shows a viewer the count and does not fetch manager-only connection details', () => {
    const cache = new QueryClient(); const request = vi.spyOn(portApi, 'connectionFactsQuery');
    show(<QueryClientProvider client={cache}><PortConnections opening={opening('place')} manage={false} /></QueryClientProvider>);
    expect(screen.getByText(/3 connections/)).toBeVisible();
    expect(cache.isFetching()).toBe(0);
    expect(request).toHaveBeenCalledOnce();
    cache.clear();
  });
  it('reads manager connection facts only after the disclosure opens',async()=>{
    vi.restoreAllMocks();
    const nativeOptions=portApi.connectionFactsQuery;
    const read=vi.fn(()=>Promise.reject(new Error('The connection read was refused.')));
    const query=vi.spyOn(portApi,'connectionFactsQuery').mockImplementation((...args)=>({...nativeOptions(...args),queryFn:read}));
    const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
    const view=show(<QueryClientProvider client={cache}><PortConnections opening={{...opening('place'),id:'s8-quiet-connections'}} manage /></QueryClientProvider>);
    try{
      expect(read).not.toHaveBeenCalled();expect(cache.isFetching()).toBe(0);
      await userEvent.click(screen.getByText(/Who is connected \(/));
      await waitFor(()=>expect(read).toHaveBeenCalledTimes(1));
      expect(query).toHaveBeenCalled();
    }finally{view.unmount();query.mockRestore();cache.clear();}
  });
});
