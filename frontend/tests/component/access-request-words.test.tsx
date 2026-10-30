/** Access requests use the same translated permission names on their inbox card, page card and waiting line. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import { AccessRequest } from '@/features/access/AccessRequest';
import { RequestCard } from '@/features/access/RequestCard';
import { AccessCard } from '@/features/approvals/Access';
import type { AccessRequestGetReply, Approval, MissingAccess, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ADA, BEA, CATALOGUE, TOPIC } from '../unit/general-access-fixtures';
import { CATALOGUE as NATIVE_CATALOGUE } from '../doubles/catalogue';
import { show } from './support';

const permissions = ['chat.view', 'doc.view', 'thread.view'];
const approval: Extract<Approval, { kind: 'access' }> = {
  id: 'request', batch_id: 'batch', kind: 'access', state: 'pending', title: 'Review Release',
  object: { kind: 'thread', id: 'release', name: 'Release' }, node: TOPIC, requester: { user: BEA },
  request: { permissions, reason: 'Read the release discussion.', holder: { kind: 'user', user: BEA }, options: null, for_thread: null },
  required_permission: 'access.manage', can_decide: true, approver_scope: null, created_at: '2026-10-15T21:31:00Z',
  decided_at: null, decided_automatically: false, decided_by: null, decision: null, expires_at: null, item_id: null, thread_id: null,
};
const missing: MissingAccess = { object: { kind: 'thread', id: 'release', name: 'Release', owner: ADA }, permissions,
  options: null, ask: [ADA], ask_more: 0, override: false, closed: null };
const calls = vi.hoisted(() => ({ request: vi.fn<typeof AccessApi.requestAccess>(),read:vi.fn<(batch:string)=>Promise<AccessRequestGetReply>>() }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(),
  // A server that supplies only its keys as labels cannot leak them into the reader's screen.
  permissionsQuery: { queryKey: ['request-word-catalogue'], queryFn: () => Promise.resolve({ ...CATALOGUE.reply,
    permissions: NATIVE_CATALOGUE.permissions.map((entry) => ({ ...entry, label: entry.name })),
  }) },
  requestAccess: (...args: Parameters<typeof AccessApi.requestAccess>) => calls.request(...args),
  accessRequestQuery: (batch: string) => ({ queryKey: ['request-words', batch], queryFn: () => calls.read(batch) }),
}));
vi.mock('@/realtime/live',async original=>({...await original(),useSocketStatus:()=>({phase:'ready'})}));
vi.mock('@/realtime/stores/tree', () => ({ useNodePath: () => ({ path: [] }) }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: UserRef }) => <button data-person-chip={user.id}>{user.display_name}</button> }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const oldTheme = document.documentElement.dataset.theme;
const oldDark = document.documentElement.classList.contains('dark');
beforeEach(() => { sessionStorage.clear(); vi.clearAllMocks(); calls.request.mockResolvedValue({ batch_id: 'batch', approvals: [approval] });calls.read.mockResolvedValue({batch_id:'batch',approvals:[approval],complete:false}); });
afterEach(async () => {
  cleanup(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', oldDark);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme;
  await setLocale('en');
});

for (const [locale, theme] of modes) {
  const names = locale === 'en' ? ['View chat', 'View document', 'View Thread'] as const : ['查看聊天', '查看文档', '查看会话'] as const;
  it(`a private reference never names approvers even if its refusal supplied them (${locale}/${theme})`,async()=>{
    await setLocale(locale);useAppearance.setState({theme,resolvedTheme:theme});document.documentElement.dataset.theme=theme;
    const view=show(<AccessRequest missing={[missing]} objectId="release" fixed privateReference sharedLabel="Shared release"/>);
    expect(view.container).toHaveTextContent(i18n.t('people.reference.ownersAsked'));
    expect(view.container).toHaveTextContent(i18n.t('access.request.asksFor',{access:names.join(', ')}));
    expect(view.container).not.toHaveTextContent(ADA.display_name);
    expect(screen.getByRole('button',{name:i18n.t('access.request.send')})).toBeEnabled();
  });
  for(const outcome of ['denied','partial'] as const)it(`the requester sees the common ${outcome} status with its deciding person and time (${locale}/${theme})`,async()=>{
    await setLocale(locale);useAppearance.setState({theme,resolvedTheme:theme});document.documentElement.dataset.theme=theme;
    const at='2026-10-26T12:00:00Z';
    const decided={...approval,state:outcome==='denied'?'denied' as const:'approved' as const,decided_at:at,decided_by:ADA,
      decision:outcome==='denied'?{approve:false,note:'Use the shared copy'}:{approve:true,permissions:['chat.view'],expires_at:'2026-11-02T12:00:00Z'}};
    calls.read.mockResolvedValue({batch_id:'batch',approvals:[decided],complete:true});
    show(<AccessRequest missing={[missing]} objectId="release" fixed/>);
    await userEvent.click(screen.getByRole('button',{name:i18n.t('access.request.send')}));
    const row=await screen.findByRole('listitem');const status=await within(row).findByRole('status');
    expect(within(status).getByRole('button',{name:ADA.display_name})).toHaveAttribute('data-person-chip',ADA.id);
    expect(status.querySelector('time')).toHaveAttribute('datetime',at);
    if(outcome==='denied')expect(status).toHaveTextContent('Use the shared copy');
    else{
      expect(status).toHaveTextContent(i18n.t('approvals.quick.given'));
      expect(row).toHaveTextContent(i18n.t('access.request.line.approvedFewer',{what:`Release · ${names.join(', ')}`,given:names[0],left:names.slice(1).join(', ')}));
    }
  });
  for (const [kind, permission] of [['environment', 'terminal.manage'], ['provider', 'provider.use']] as const) it(`names the ${permission} choice from the permission catalogue in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const item = { ...approval, object: { kind, id: 'resource', name: 'Workshop' }, request: { ...approval.request, permissions: [permission] } };
    const view = show(<AccessCard items={[item]} settled={new Map<string, Approval>()} live onSettled={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button',{name:i18n.t('approvals.quick.other')}));
    const choice = await screen.findByRole('checkbox', { name: i18n.t(`access.permission.${permission}`) });
    expect(choice).toBeChecked();
    expect(view.container).not.toHaveTextContent(permission);
    expect(view.container).not.toHaveTextContent(i18n.t('resourceAccessWords.unavailable'));
    await userEvent.click(choice);
    expect(choice).not.toBeChecked();
  });
  for (const surface of ['inbox', 'page'] as const) it(`names each permission on the ${surface} access card in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const view = show(surface === 'inbox'
      ? <AccessCard items={[approval]} settled={new Map<string, Approval>()} live onSettled={vi.fn()} />
      : <ul><RequestCard approval={approval} /></ul>);
    await userEvent.click(await screen.findByRole('button',{name:i18n.t('approvals.quick.other')}));
    for (const name of names) expect(await screen.findByRole('checkbox', { name })).toBeChecked();
    expect(screen.getAllByRole('checkbox')).toHaveLength(permissions.length);
    for (const permission of permissions) expect(view.container).not.toHaveTextContent(permission);
    const documentAccess = screen.getByRole('checkbox', { name: names[1] });
    await userEvent.click(documentAccess);
    expect(documentAccess).not.toBeChecked();
    expect(screen.getByRole('checkbox', { name: names[0] })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: names[2] })).toBeChecked();
  });

  it(`keeps the original permissions on the wire and their words in the waiting line in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const view = show(<AccessRequest missing={[missing]} objectId="release" fixed />);
    await user.type(screen.getByRole('textbox', { name: i18n.t('access.request.reason') }), 'Read the release discussion.');
    await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
    await waitFor(() => expect(calls.request).toHaveBeenCalledOnce());
    expect(calls.request.mock.calls[0]?.[0]).toEqual({ items: [{ object: { kind: 'thread', id: 'release' }, permissions }], reason: 'Read the release discussion.' });
    const sentence = i18n.t('access.request.line.pendingNobody', { what: `Release · ${names.join(', ')}` });
    const waiting = (await screen.findByText(sentence, { exact: true })).closest('[role="status"]');
    if (waiting === null) throw new Error('The pending request has no live status');
    for (const name of names) expect(waiting).toHaveTextContent(name);
    for (const permission of permissions) expect(view.container).not.toHaveTextContent(permission);
    expect(waiting).toHaveTextContent(sentence);
    expect(screen.getByRole('button', { name: i18n.t('access.request.cancel') })).toBeEnabled();
  });
}
