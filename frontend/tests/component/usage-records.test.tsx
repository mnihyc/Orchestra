/** A usage record leads with the thread, omits repeated scope and never exposes its routing model. */
import type { ReactNode } from 'react';
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { Records } from '@/features/usage/Records';
import type { UsageEvent } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { requestRow } from '../doubles/request-row';
import { show } from './support';

const shown = vi.hoisted(() => ({ embeddings: false, row: null as UsageEvent | null }));
vi.mock('@/api/usage',()=>({eventsQuery:()=>({queryKey:['records-test'],initialPageParam:null,queryFn:()=>Promise.resolve({items:[shown.embeddings ? { ...requestRow(), thread:null, purpose:'embeddings' } : shown.row ?? requestRow()],next_cursor:null}),getNextPageParam:()=>null})}));
vi.mock('@tanstack/react-router',()=>({Link:({params,search,children}:{params:{threadId:string};search:{turn?:number};children:ReactNode})=><a href={`/thread/${params.threadId}?turn=${search.turn ?? ''}`}>{children}</a>}));
const appearance = useAppearance.getState();
afterEach(async()=>{shown.row=null;useAppearance.setState(appearance);await setLocale('en');});

it('keeps a short numbered-turn link, place context once, and display names at narrow widths too',async()=>{
  shown.embeddings = false;
  const row=requestRow();
  show(<Records scope={{kind:'node',id:row.project.id}} search={{}} span={{from:'2026-10-15',to:'2026-10-15',days:1,range:'custom'}} depth="reports"/>);
  const table=await screen.findByRole('table',{name:'Records'});
  expect(within(table).getByRole('link',{name:'Repair redirects'})).toHaveAttribute('href','/thread/00000000-0000-4000-8000-000000000104?turn=7');
  expect(within(table).getAllByText('Repair redirects')).toHaveLength(1);
  expect(table).toHaveTextContent('Sign in');expect(table).not.toHaveTextContent('Web');
  expect(table).toHaveTextContent('Model A');expect(table).not.toHaveTextContent('model-a');
});

it('names embeddings separately from summaries in usage records without a thread', async () => {
  shown.embeddings = true;
  show(<Records scope={{kind:'me'}} search={{}} span={{from:'2026-10-15',to:'2026-10-15',days:1,range:'custom'}} depth="reports"/>);
  const table = await screen.findByRole('table', {name:'Records'});
  expect(table).toHaveTextContent('Embeddings');
  expect(table).not.toHaveTextContent('A summary of the topic');
});

for(const locale of ['en','zh-CN'] as const)it(`shows a third program’s supplied name in record details (${locale})`,async()=>{
  await setLocale(locale);useAppearance.setState({locale});shown.embeddings=false;
  const named={harness:{kind:'research-agent',name:'Research assistant',names:{'zh-CN':'研究助理'}}};
  shown.row={...requestRow({harness_kind:'research-agent'}),...named};
  show(<Records scope={{kind:'me'}} search={{}} span={{from:'2026-10-15',to:'2026-10-15',days:1,range:'custom'}} depth="reports"/>);
  await userEvent.click(await screen.findByRole('button',{name:i18n.t('usage.records.aboutRecord')}));
  const details=screen.getByRole('dialog',{name:i18n.t('usage.records.aboutRecord')});
  expect(within(details).getByText(locale==='en'?'Research assistant':'研究助理')).toBeVisible();
  expect(details).not.toHaveTextContent('research-agent');
});


for(const locale of ['en','zh-CN'] as const)it(`shows retained request facts in the row and More about this record (${locale})`,async()=>{
  await setLocale(locale);useAppearance.setState({locale});shown.embeddings=false;
  const timing=(await import('../doubles/performance')).timingFixture;
  const base=requestRow();shown.row=requestRow({thread_owner:base.user,tool_calls:2,tool_ms:400,timing:timing({
    reported_tokens:{input_tokens:100,cached_input_tokens:20,cache_write_tokens:10,output_tokens:64,reasoning_tokens:0,total_tokens:164},
    outcome:{state:'told',value:'rate_limited'},upstream_status:429,attempt:3,provider_request_id:'provider-local',
  })});
  show(<Records scope={{kind:'me'}} search={{}} span={{from:'2026-10-15',to:'2026-10-15',days:1,range:'custom'}} depth="reports"/>);
  const table=await screen.findByRole('table',{name:i18n.t('usage.records.title')});
  for(const key of ['uncached','cached','cacheWrite','status','attempt'] as const)expect(within(table).getByText(i18n.t(`usageRequest.${key}`))).toBeVisible();
  await userEvent.click(within(table).getByRole('button',{name:i18n.t('usage.records.aboutRecord')}));
  const details=screen.getByRole('dialog',{name:i18n.t('usage.records.aboutRecord')});
  for(const key of ['uncached','cached','cacheWrite','status','attempt','tools','toolTime','providerId','adapterId','owner'] as const)expect(within(details).getByText(i18n.t(`usageRequest.${key}`))).toBeVisible();
  expect(details).toHaveTextContent('provider-local');expect(details).toHaveTextContent('request-1');expect(details).toHaveTextContent('Dana');
  expect(details).toHaveTextContent('429');expect(details).toHaveTextContent('70');
});
