/** A manager can restart one failed server from the same list used on the environment panel and page. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { McpServers } from '@/features/resources/environments/McpServers';
import type { Environment, EnvironmentMcp, Job } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const { read, restart, jobRead } = vi.hoisted(() => ({ read: vi.fn<() => Promise<EnvironmentMcp>>(), restart: vi.fn<(id: string,name: string) => Promise<Job>>(), jobRead: vi.fn<() => Promise<Job>>() }));
vi.mock('@/api/environments', () => ({ environmentMcpKey: (id: string) => ['environment-mcp',id], environmentMcpQuery: (id: string) => ({ queryKey: ['environment-mcp',id], queryFn: read }), restartEnvironmentMcpServer: restart }));
vi.mock('@/api/forks', () => ({ jobQuery: (id: string) => ({ queryKey: ['job',id], queryFn: jobRead }) }));
afterEach(() => vi.clearAllMocks());

function list(): EnvironmentMcp {
  return { applies:'next_turn', found:[], import:{allowed:false,reason:'policy'}, servers:[
    { server:{id:'flaky',name:'flaky',address:'team.flaky',machine:null,own:true},from:null,state:'failed',reason:'The server ended (exit status: 7): before crash [masked]',tools:null },
    { server:{id:'steady',name:'steady',address:'team.steady',machine:null,own:true},from:null,state:'running',reason:null,tools:3 }
  ] };
}

function job(): Job {
  return { id:'restart-one',kind:'mcp_restart',node_id:null,environment_id:'environment',provider_id:null,batch_id:null,state:'succeeded',result:{name:'flaky'},error:null,log:null,created_by:null,created_at:'2026-10-14T19:00:00Z',started_at:'2026-10-14T19:00:00Z',finished_at:'2026-10-14T19:00:01Z', progress:{phase:'record',phases:[],percent:100,files_done:0,files_total:null,bytes_done:0,bytes_total:null,rate_bps:null,eta_seconds:null,message:null} };
}

function draw(manage: boolean, compact: boolean) {
  const environment: Pick<Environment,'id'|'access'|'status'> = { id:'environment',status:'running',access:{permissions:manage?['environment.view','environment.manage']:['environment.view'],administers:false,participating:true,admin_only:[],through:[]} };
  const client = new QueryClient({ defaultOptions:{queries:{retry:false},mutations:{retry:false}} });
  show(<QueryClientProvider client={client}><McpServers environment={environment} compact={compact} /></QueryClientProvider>);
  return client;
}

describe('one MCP server restart', () => {
  it('receives an own server’s library figures with the scoped list read',async()=>{
    const current=list();const server=current.servers[0];
    if(server===undefined)throw new Error('The fixture has an own server');
    server.library={subject:{kind:'resource',object:{kind:'mcp_server',id:server.server.id}},as_of:'2026-10-27T00:00:00Z',dependents_complete:true,dependents:[],uses:{kind:'mcp_call',coverage:{state:'partial',since:'2026-10-01T00:00:00Z'},all_time:12,last_30_days:3,people:1,people_30_days:1,last_used_at:'2026-10-27T00:00:00Z'}};
    read.mockResolvedValue(current);draw(false,true);
    expect(await screen.findByText('Used 12 calls')).toBeVisible();expect(read).toHaveBeenCalledTimes(1);expect(restart).not.toHaveBeenCalled();
  });
  it('replaces stale server facts with one failed-read message and retries the actual list', async () => {
    read.mockResolvedValue(list());
    const cache = draw(false, true);
    await screen.findByText('steady');
    read.mockRejectedValueOnce(new Error('No current server response'));
    await act(async () => { await cache.refetchQueries({ queryKey: ['environment-mcp', 'environment'], exact: true }); });
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(screen.queryByText('steady')).not.toBeInTheDocument();
    expect(screen.queryByText('flaky')).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('mcpServers.empty'))).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /Try again/ }));
    expect(await screen.findByText('steady')).toBeVisible();
    expect(read).toHaveBeenCalledTimes(3);
  });
  for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps the panel's states visible and its explanation behind one question at ${locale}/${theme}`, async () => {
    const beforeTheme = document.documentElement.getAttribute('data-theme');
    try {
      await setLocale(locale); document.documentElement.dataset.theme = theme;
      read.mockResolvedValue(list()); draw(false, true);
      expect(await screen.findByText('steady')).toBeVisible();
      expect(screen.getByText(i18n.t('mcpServers.states.running'))).toBeVisible();
      const explanation = i18n.t('mcpServers.applies');
      expect(screen.queryByText(explanation)).not.toBeInTheDocument();
      const question = i18n.t('help.hint.label', { term: i18n.t('mcpServers.title') });
      expect(screen.getAllByRole('button', { name: question })).toHaveLength(1);
      await userEvent.click(screen.getByRole('button', { name: question }));
      expect(screen.getByText(explanation)).toBeVisible();
      expect(read).toHaveBeenCalledTimes(1); expect(restart).not.toHaveBeenCalled();
    } finally {
      if (beforeTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.dataset.theme = beforeTheme;
      await setLocale('en');
    }
  });
  for (const compact of [false,true]) it(`shows the masked exit and restarts only the selected server (${compact ? 'panel' : 'page'})`, async () => {
    let current = list();
    read.mockImplementation(() => Promise.resolve(current));
    jobRead.mockResolvedValue(job());
    restart.mockImplementation((id,name) => { expect([id,name]).toEqual(['environment','flaky']); current = { ...current,servers:current.servers.map((server) => server.server.name === name ? {...server,state:'running',reason:null} : server) }; return Promise.resolve(job()); });
    draw(true,compact);
    const flaky = (await screen.findByText('flaky')).closest('li')!;
    expect(within(flaky).getByText('Failed')).toBeVisible();
    expect(within(flaky).getByText(/exit status: 7/)).toHaveTextContent('[masked]');
    const steady = screen.getByText('steady').closest('li')!;
    expect(within(steady).getByText('Running')).toBeVisible();
    await userEvent.click(within(flaky).getByRole('button',{name:'Restart'}));
    await waitFor(() => expect(within(flaky).getByText('Running')).toBeVisible());
    expect(restart).toHaveBeenCalledTimes(1);
    expect(within(steady).getByText('Running')).toBeVisible();
  });
  it('lets a viewer read states and failure output, with no Restart act', async () => {
    read.mockResolvedValue(list());
    draw(false,true);
    await screen.findByText('flaky');
    expect(screen.queryByRole('button',{name:'Restart'})).not.toBeInTheDocument();
  });
});
