/** Update notices keep current work, measured impact and an independently inspectable actor together. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { NotificationRow } from '@/features/inbox/NotificationRow';
import type { Notification } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../doubles/chat-rows';
import { show } from './support';

vi.mock('@tanstack/react-router',async(load)=>({...await load<object>(),useNavigate:()=>vi.fn()}));
afterEach(async()=>{cleanup();await setLocale('en');});

for(const locale of ['en','zh-CN'] as const) it(`keeps the actor card outside the work link (${locale})`,async()=>{
  await setLocale(locale);
  const thread={id:'affected-thread',kind:'thread' as const,name:'Current work',redacted:false};
  const parameters={reason:'requested',affected_threads:2};
  const row:Notification={id:'update-notice',kind:'environment.update',title:'Stored title',body:'Stored body',actor:ADA,node:thread,thread,thread_id:thread.id,path:[thread],approval_id:null,chat_id:null,url:`/thread/${thread.id}`,data:parameters,details:{v:1,action:'environment.update',parameters},created_at:'2026-10-26T00:00:00Z',read_at:null,inbox_seq:1,group:null,waiting_for_me:false};
  const view=show(<ul><NotificationRow row={row} /></ul>);
  expect(screen.getByText('Current work',{exact:true})).toBeVisible();
  expect(screen.getByText(i18n.t('programVersions.noticeThreads',{count:2,number:'2'}))).toBeVisible();
  const actor=view.container.querySelector('[data-inbox-person]');
  if(actor===null) throw new Error('The update notice must name its actor');
  expect(within(actor as HTMLElement).getByText(ADA.display_name,{exact:true})).toBeVisible();
  expect(actor.closest('a')).toBeNull();
  await waitFor(()=>expect(actor.querySelector('button')).not.toBeNull());
});
