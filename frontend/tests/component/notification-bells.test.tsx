/** Bells expose the served level, a visible Off and an unmistakable selection without losing newer facts. */
import { act,cleanup,screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach,beforeEach,describe,expect,it,vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as ChatsApi from '@/api/chats';
import type * as ExceptionsApi from '@/api/notificationExceptions';
import type * as PeopleApi from '@/api/people';
import { Bell } from '@/features/chat/Bell';
import { DiscussionBell } from '@/features/people/DiscussionBell';
import { SubscriptionBell } from '@/features/thread/SubscriptionBell';
import type { Document as HeardDocument,NotificationExceptionChanged,Thread } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { statesWriter,useChatState } from '@/realtime/stores/chat';
import { threadViewOf,threadWriter,useThreadPart } from '@/realtime/stores/thread';
import { chat,state } from '../doubles/chat-rows';
import { memberAccess } from '../doubles/fixtures';
import { show } from './support';

const calls=vi.hoisted(()=>({change:vi.fn<typeof ExceptionsApi.setNotificationException>(),chat:vi.fn<typeof ChatsApi.setChatState>(),discussion:vi.fn<typeof PeopleApi.setDiscussionLevel>()}));
vi.mock('@/api/notificationExceptions',async(original)=>({...await original<typeof ExceptionsApi>(),setNotificationException:calls.change}));
vi.mock('@/api/chats',async(original)=>({...await original<typeof ChatsApi>(),setChatState:calls.chat}));
vi.mock('@/api/people',async(original)=>({...await original<typeof PeopleApi>(),setDiscussionLevel:calls.discussion}));
// Outside delivery is independently covered; opening a level menu must not obscure these assertions with unrelated reads.
vi.mock('@/features/settings/personal/away/Status',()=>({OutsideMenu:()=>null}));
const ID='notification-thread';
const initial:Thread={id:ID,kind:'thread',name:'Current name',state:'idle',seq:1,access:{...memberAccess,subscription:'off',effective_subscription:'off',inherited_subscription:'all'}} as Thread;
function ThreadBell({id=ID}:{id?:string}){const held=useThreadPart(id,(view)=>view.thread);return held===null?null:<><h1>{held.name}</h1><SubscriptionBell thread={held}/></>;}
const room=chat();
function ChatBell(){return <Bell chat={room} state={useChatState(room.id)}/>;}
const accepted=(level:'all'|'important'|'off'|null):NotificationExceptionChanged=>({access:{...initial.access,subscription:level,effective_subscription:level ?? 'all'},exceptions:{items:[],next_cursor:null}});

beforeEach(()=>{calls.change.mockReset();calls.chat.mockReset();calls.discussion.mockReset();threadWriter.evict(ID);statesWriter.clear();});
afterEach(async()=>{cleanup();threadWriter.evict(ID);threadWriter.evict('another-notification-thread');statesWriter.clear();document.documentElement.removeAttribute('data-theme');await setLocale('en');});

it('serializes native page-changing writes from separate open bells so an older page cannot arrive last',async()=>{
  const other='another-notification-thread';
  threadWriter.reset(ID,{thread:structuredClone(initial)},false);
  threadWriter.reset(other,{thread:{...structuredClone(initial),id:other,name:'Another thread'}},false);
  let finish:(reply:NotificationExceptionChanged)=>void=()=>undefined;
  calls.change.mockImplementationOnce(()=>new Promise((resolve)=>{finish=resolve;}));
  calls.change.mockResolvedValue(accepted('important'));
  show(<><section aria-label="First thread"><ThreadBell/></section><section aria-label="Second thread"><ThreadBell id={other}/></section></>);
  const user=userEvent.setup();
  await user.click(within(screen.getByRole('region',{name:'First thread'})).getByRole('button',{name:i18n.t('thread.bell.label')}));
  await user.click(screen.getByRole('menuitemradio',{name:i18n.t('choiceCatalog.subscriptionLevel.all.name')}));
  await user.click(within(screen.getByRole('region',{name:'Second thread'})).getByRole('button',{name:i18n.t('thread.bell.label')}));
  await user.click(screen.getByRole('menuitemradio',{name:i18n.t('choiceCatalog.subscriptionLevel.important.name')}));
  expect(calls.change).toHaveBeenCalledTimes(1);
  act(()=>{finish(accepted('all'));});
  await waitFor(()=>expect(calls.change).toHaveBeenCalledTimes(2));
  expect(calls.change.mock.calls.map(([id])=>id)).toEqual([ID,other]);
  await waitFor(()=>expect(threadViewOf(other).thread?.access.effective_subscription).toBe('important'));
  expect(threadViewOf(ID).thread?.access.effective_subscription).toBe('all');
});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)describe(`${locale}/${theme}`,()=>{
  async function drawThread(){await setLocale(locale);document.documentElement.dataset.theme=theme;threadWriter.reset(ID,{thread:structuredClone(initial)},false);show(<ThreadBell/>);return {t:i18n.getFixedT(locale),user:userEvent.setup()};}

  it('shows Off on the bell, a real checkmark/fill, and Default from above rather than the own Off',async()=>{
    const {t,user}=await drawThread();
    const bell=screen.getByRole('button',{name:t('thread.bell.label')});
    expect(bell).toHaveTextContent(t('notificationBell.level.off'));
    expect(bell).toHaveAccessibleDescription(t('notificationBell.level.off'));
    expect(bell.querySelector('svg.lucide-bell-off')).not.toBeNull();
    await user.click(bell);
    const selected=screen.getByRole('menuitemradio',{name:t('choiceCatalog.subscriptionLevel.off.name')});
    expect(selected).toHaveAttribute('aria-checked','true');
    expect(selected.querySelector('svg.lucide-check')).not.toBeNull();
    expect(selected.querySelector('svg.lucide-dot')).toBeNull();
    expect(selected).toHaveClass('data-[state=checked]:bg-accent-subtle');
    const inherited=screen.getByRole('menuitemradio',{name:t('notificationBell.defaultWithLevel',{level:t('notificationBell.level.all')})});
    expect(inherited).toHaveAccessibleName(t('notificationBell.defaultWithLevel',{level:t('notificationBell.level.all')}));
    expect(screen.getByRole('menu').querySelectorAll('[data-choice-description]')).toHaveLength(0);
    expect(screen.queryByText(t('notifications.levels.off.what'))).not.toBeInTheDocument();
    calls.change.mockResolvedValue(accepted(null));
    await user.click(inherited);
    await waitFor(()=>expect(bell).toHaveAccessibleDescription(t('notificationBell.inherited',{level:t('notificationBell.level.all')})));
    expect(bell).not.toHaveTextContent(t('notificationBell.level.off'));
    expect(bell.querySelector('svg.lucide-bell-ring')).not.toBeNull();
    expect(calls.change).toHaveBeenCalledExactlyOnceWith(ID,null,null);
  });

  it('keeps focus and the native value while waiting, and a refusal cannot revert newer thread facts',async()=>{
    const {t,user}=await drawThread();
    let refuse:(error:Error)=>void=()=>undefined;
    calls.change.mockImplementation(()=>new Promise((_yes,no)=>{refuse=no;}));
    const bell=screen.getByRole('button',{name:t('thread.bell.label')});
    await user.click(bell);await user.click(screen.getByRole('menuitemradio',{name:t('choiceCatalog.subscriptionLevel.all.name')}));
    await waitFor(()=>expect(bell).toHaveAttribute('aria-busy','true'));
    expect(bell).toHaveTextContent(t('notificationBell.level.off'));
    await user.click(bell);expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    act(()=>{threadWriter.patch(ID,(held)=>({...held,name:'Renamed while waiting',state:'running'}));refuse(new Error('The level could not be saved.'));});
    await waitFor(()=>expect(bell).not.toHaveAttribute('aria-busy'));
    expect(screen.getByRole('heading',{name:'Renamed while waiting'})).toBeVisible();
    expect(threadViewOf(ID).thread?.state).toBe('running');
    expect(bell).toHaveTextContent(t('notificationBell.level.off'));
    expect(calls.change).toHaveBeenCalledTimes(1);
  });

  it('shows Important on the compact thread bell only after its native acknowledgement',async()=>{
    const {t,user}=await drawThread();
    let accept:(reply:NotificationExceptionChanged)=>void=()=>undefined;
    calls.change.mockImplementationOnce(()=>new Promise((resolve)=>{accept=resolve;}));
    const bell=screen.getByRole('button',{name:t('thread.bell.label')});
    await user.click(bell);
    await user.click(screen.getByRole('menuitemradio',{name:t('choiceCatalog.subscriptionLevel.important.name')}));
    await waitFor(()=>expect(bell).toHaveAttribute('aria-busy','true'));
    expect(bell).toHaveTextContent(t('notificationBell.level.off'));
    expect(bell).not.toHaveTextContent(t('notificationBell.level.important'));
    act(()=>accept(accepted('important')));
    await waitFor(()=>expect(bell).toHaveTextContent(t('notificationBell.level.important')));
    await waitFor(()=>expect(bell).not.toHaveAttribute('aria-busy'));
    expect(bell).toHaveAccessibleDescription(t('notificationBell.level.important'));
    await user.unhover(bell);await user.hover(bell);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(t('thread.bell.label'));
    expect(bell).toHaveAccessibleDescription(t('notificationBell.level.important'));
    await user.unhover(bell);
    expect(bell).not.toHaveTextContent(t('notificationBell.level.off'));
    expect(bell.querySelector('svg.lucide-bell')).not.toBeNull();
    expect(bell).toHaveClass('h-(--control-sm)');
    expect(calls.change).toHaveBeenCalledExactlyOnceWith(ID,'important',null);
    await user.click(bell);
    const selected=screen.getByRole('menuitemradio',{name:t('choiceCatalog.subscriptionLevel.important.name')});
    expect(selected).toHaveAttribute('aria-checked','true');
    expect(selected.querySelector('svg.lucide-check')).not.toBeNull();
  });

  it('keeps a chat’s served default and native Off glyph instead of a generic notification-settings icon',async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale),user=userEvent.setup();
    statesWriter.put(state(room.id,{level:'off',effective_level:'off',default_level:'mentions'}));
    show(<ChatBell/>);
    const bell=screen.getByRole('button',{name:t('actions.notificationSettings.label')});
    expect(bell.querySelector('svg.lucide-bell-off')).not.toBeNull();expect(bell).toHaveTextContent(t('notificationBell.level.off'));
    calls.chat.mockResolvedValue(state(room.id,{level:null,effective_level:'mentions',default_level:'mentions'}));
    await user.click(bell);
    const selected=screen.getByRole('menuitemradio',{name:t('choiceCatalog.subscriptionLevel.off.name')});
    expect(selected.querySelector('svg.lucide-check')).not.toBeNull();
    const inherited=screen.getByRole('menuitemradio',{name:t('notificationBell.defaultWithLevel',{level:t('notificationBell.level.mentions')})});
    expect(screen.getByRole('menu').querySelectorAll('[data-choice-description]')).toHaveLength(0);
    await user.click(inherited);
    await waitFor(()=>expect(bell).toHaveAccessibleDescription(t('notificationBell.inherited',{level:t('notificationBell.level.mentions')})));
    expect(calls.chat).toHaveBeenCalledExactlyOnceWith(room.id,{level:null});
    expect(bell).not.toHaveTextContent(t('notificationBell.level.off'));
  });

  it('marks the current document-discussion choice with the same full check',async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale),user=userEvent.setup();
    show(<DiscussionBell doc={{id:'document',discussion_level:'involved'} as HeardDocument}/>);
    await user.click(screen.getByRole('button',{name:t('people.discussionBell')}));
    const selected=screen.getByRole('menuitemradio',{name:t('choiceCatalog.discussionLevel.involved.name')});
    expect(selected).toHaveAttribute('aria-checked','true');expect(selected.querySelector('svg.lucide-check')).not.toBeNull();
    expect(within(selected).getByText(t('choiceCatalog.discussionLevel.involved.name'),{exact:true})).toBeVisible();
  });
});


it('keeps a refused discussion change in its anchored card and returns focus to the bell on Escape', async () => {
  const user = userEvent.setup();
  const refusal = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Request denied', detail: 'Request denied', request_id: 'discussion-level' }, null);
  calls.discussion.mockRejectedValueOnce(refusal);
  show(<DiscussionBell doc={{ id: 'document', discussion_level: 'involved' } as HeardDocument} />);
  const bell = screen.getByRole('button', { name: i18n.t('people.discussionBell') });
  await user.click(bell);
  await user.click(screen.getByRole('menuitemradio', { name: i18n.t('choiceCatalog.discussionLevel.all.name') }));
  await waitFor(() => expect(calls.discussion).toHaveBeenCalledExactlyOnceWith('document', 'all'));
  expect(await screen.findByRole('dialog', { name: i18n.t('people.discussionBell') })).toHaveTextContent(refusal.message);
  expect(bell).toHaveAccessibleDescription(i18n.t('notificationBell.level.involved'));
  expect(calls.discussion).toHaveBeenCalledExactlyOnceWith('document', 'all');
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  await waitFor(() => expect(bell).toHaveFocus());
});
