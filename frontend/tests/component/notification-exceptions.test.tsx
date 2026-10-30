/** A person's place exceptions are editable here, with Default, Undo and one acknowledged request. */
import { cleanup,screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach,beforeEach,describe,expect,it,vi } from 'vitest';
import { NotificationExceptions } from '@/features/settings/personal/NotificationExceptions';
import { setNotificationException } from '@/api/notificationExceptions';
import type { NodeRef,NotificationException,NotificationExceptionChanged,NotificationExceptionPage,SubscriptionLevel } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { memberAccess } from '../doubles/fixtures';
import { show } from './support';

const reads=vi.hoisted(()=>vi.fn<(cursor:string|null)=>Promise<NotificationExceptionPage>>());
const locator=vi.hoisted(()=>({read:vi.fn(),open:vi.fn()}));
vi.mock('@/features/chat/open',()=>({useOpenPath:()=>locator.open}));
vi.mock('@/api/search',()=>({namesQuery:({q}:{q:string})=>({queryKey:['notification-place-names',q],queryFn:()=>{locator.read(q);return Promise.resolve([{id:'topic',kind:'topic',name:'Login',path:[],href:'/topic/topic'}]);}})}));
vi.mock('@/api/notificationExceptions',()=>({notificationExceptionsKey:['notification-exceptions'],setNotificationException:vi.fn(),
  notificationExceptionsQuery:(cursor:string|null)=>({queryKey:['notification-exceptions',cursor],queryFn:()=>reads(cursor)})}));
vi.mock('@/features/navigation/places',()=>({PlaceLink:({place,children,className}:{place:NodeRef;children:ReactNode;className?:string})=><a href={'/place/'+place.id} className={className}>{children}</a>}));
const root:NodeRef={id:'root',kind:'org',name:'Acme',redacted:false};
const topic:NodeRef={id:'topic',kind:'topic',name:'Login',redacted:false};
const initial:NotificationException={node:topic,path:[root],level:'off',default_level:'important',default_from:root};
let rows:NotificationException[]=[];
function reply(level:SubscriptionLevel|null,items:NotificationException[]=rows):NotificationExceptionChanged {
  return {access:{...memberAccess,subscription:level,effective_subscription:level ?? initial.default_level},exceptions:{items,next_cursor:null}};
}
beforeEach(()=>{
  vi.clearAllMocks();rows=[structuredClone(initial)];reads.mockImplementation(()=>Promise.resolve({items:rows,next_cursor:null}));
  vi.mocked(setNotificationException).mockImplementation((_id,level)=>{
    rows=level===null || level===initial.default_level?[]:[{...initial,level}];return Promise.resolve(reply(level));
  });
});
afterEach(async()=>{cleanup();document.documentElement.removeAttribute('data-theme');await setLocale('en');});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'])describe(`${locale}/${theme}`,()=>{
  async function open(){await setLocale(locale);document.documentElement.dataset.theme=theme;show(<NotificationExceptions/>);await screen.findByRole('link',{name:'Login'});return {t:i18n.getFixedT(locale),user:userEvent.setup()};}
  it('changes a row, resets to its true inherited level and supports Undo without rereading the page',async()=>{
    const {t,user}=await open();const label=t('notificationExceptions.level',{place:'Login'});
    await user.click(screen.getByRole('combobox',{name:label}));
    const inherited=t('notificationExceptions.default',{level:t('notifications.levels.important.name')});
    expect(screen.getByRole('option',{name:inherited})).toHaveAccessibleDescription(t('notificationExceptions.inherited',{place:'Acme'}));
    await user.click(screen.getByRole('option',{name:t('notifications.levels.all.name')}));
    await waitFor(()=>expect(screen.getByRole('combobox',{name:label})).toHaveTextContent(t('notifications.levels.all.name')));
    expect(setNotificationException).toHaveBeenCalledExactlyOnceWith('topic','all',null);expect(reads).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('combobox',{name:label}));await user.click(screen.getByRole('option',{name:inherited}));
    await screen.findByText(t('notificationExceptions.empty'));expect(screen.queryByRole('link',{name:'Login'})).not.toBeInTheDocument();
    expect(setNotificationException).toHaveBeenLastCalledWith('topic',null,null);expect(reads).toHaveBeenCalledTimes(1);
    const notices=screen.getAllByRole('button',{name:t('settings.compact.undo')});await user.click(notices.at(-1)!);
    await screen.findByRole('link',{name:'Login'});expect(setNotificationException).toHaveBeenLastCalledWith('topic','all',null);
    expect(reads).toHaveBeenCalledTimes(1);
  });
  it('uses the returned descendants when changing an ancestor exposes a new exception',async()=>{
    rows=[{node:root,path:[],level:'all',default_level:'off',default_from:null}];
    await setLocale(locale);const t=i18n.getFixedT(locale);const user=userEvent.setup();show(<NotificationExceptions/>);
    const label=t('notificationExceptions.level',{place:'Acme'});
    await screen.findByRole('combobox',{name:label});
    vi.mocked(setNotificationException).mockResolvedValueOnce(reply(null,[{...initial,level:'all',default_level:'off',default_from:null}]));
    await user.click(screen.getByRole('combobox',{name:label}));await user.click(screen.getByRole('option',{name:t('notificationExceptions.default',{level:t('notifications.levels.off.name')})}));
    const list=screen.getByRole('list',{name:t('notificationExceptions.title')});
    await within(list).findByRole('link',{name:'Login'});expect(within(list).queryByRole('link',{name:'Acme'})).not.toBeInTheDocument();
    expect(reads).toHaveBeenCalledTimes(1);
  });
  it('keeps a refused row and its value so it can be retried',async()=>{
    const {t,user}=await open();vi.mocked(setNotificationException).mockRejectedValueOnce(new Error('The level was not saved'));
    const label=t('notificationExceptions.level',{place:'Login'});await user.click(screen.getByRole('combobox',{name:label}));
    await user.click(screen.getByRole('option',{name:t('notifications.levels.all.name')}));
    await screen.findByRole('alert');expect(screen.getByRole('combobox',{name:label})).toHaveTextContent(t('notifications.levels.off.name'));
    expect(screen.getByRole('link',{name:'Login'})).toBeVisible();await waitFor(()=>expect(reads).toHaveBeenCalledTimes(2));
  });
  it('uses the canonical shortest qualifier for equal place names without adding another breadcrumb',async()=>{
    rows=[initial,{...initial,node:{...topic,id:'other-topic'},path:[{...root,id:'other-root',name:'Beta'}],default_from:{...root,id:'other-root',name:'Beta'}}];
    await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale);show(<NotificationExceptions/>);
    await screen.findByRole('link',{name:'Login · Acme'});
    const list=screen.getByRole('list',{name:t('notificationExceptions.title')});
    expect(within(list).getByRole('link',{name:'Login · Acme'})).toBeVisible();
    expect(within(list).getByRole('link',{name:'Login · Beta'})).toBeVisible();
    expect(within(list).queryByText('Acme',{exact:true})).not.toBeInTheDocument();
    expect(reads).toHaveBeenCalledTimes(1);
  });
  it('replaces only the requested page and gives an emptied last page a way back, without a success refetch',async()=>{
    const later={...initial,node:{...topic,id:'later-topic',name:'Later place'}};
    reads.mockImplementation((cursor)=>Promise.resolve(cursor===null?{items:[initial],next_cursor:'page-two'}:{items:[later],next_cursor:null}));
    const {t,user}=await open();
    await user.click(screen.getByRole('button',{name:t('common.next')}));
    await screen.findByRole('link',{name:'Later place'});
    vi.mocked(setNotificationException).mockResolvedValueOnce({...reply(null,[]),page_cursor:'page-two'});
    await user.click(screen.getByRole('combobox',{name:t('notificationExceptions.level',{place:'Later place'})}));
    await user.click(screen.getByRole('option',{name:t('notificationExceptions.default',{level:t('notifications.levels.important.name')})}));
    await screen.findByText(t('notificationExceptions.pageEmpty'));
    expect(screen.queryByText(t('notificationExceptions.empty'))).not.toBeInTheDocument();
    expect(screen.getByRole('button',{name:t('common.back')})).toBeEnabled();
    expect(setNotificationException).toHaveBeenCalledExactlyOnceWith('later-topic',null,'page-two');
    expect(reads.mock.calls.map(([cursor])=>cursor)).toEqual([null,'page-two']);
  });
  it('offers one inline locator with recent authorized places and opens the selected place',async()=>{
    rows=[];await setLocale(locale);document.documentElement.dataset.theme=theme;const t=i18n.getFixedT(locale);show(<NotificationExceptions/>);
    await screen.findByText(t('notificationExceptions.empty'));
    expect(locator.read).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('combobox',{name:t('sharedWithMe.findPlaces')}));
    await userEvent.click(await screen.findByRole('option',{name:'Login'}));
    expect(locator.open).toHaveBeenCalledExactlyOnceWith('/topic/topic');expect(locator.read).toHaveBeenCalledExactlyOnceWith('');expect(reads).toHaveBeenCalledTimes(1);
  });
});
