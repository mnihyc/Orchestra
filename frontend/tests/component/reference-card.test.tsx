/** A reference chip's card: it opens under the pointer and under the keyboard's focus, shows the thing and the part it
 * names with Open, Open beside and Copy reference, copies the words an agent follows, and is not there for a reader
 * who may not read the thing. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import type * as References from '@/api/references';
import type * as Clipboard from '@/ui/clipboard';
import type * as FileSources from '@/features/files/source';
import { NamedChip } from '@/features/people/NamedChip';
import { BlockEditor, type EditorApi } from '@/features/documents/Editor';
import { DocumentMarksProvider } from '@/features/documents/marks';
import { referenceOfAddress, referenceTarget, referenceText } from '@/features/people/references';
import type { Approval, ReferencePreview } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { ReferenceCard } from '@/ui/ReferenceCard';
import { show } from './support';
import { World } from '../doubles/world';

const DOC = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e03';
const BLOCK = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e04';
const person = { id: 'ada', kind: 'human', username: 'ada', display_name: 'Ada', avatar: { text: 'A', color: 3 } } as const;
const preview: ReferencePreview = {
  kind: 'document', id: DOC, node_kind: null, title: 'Rollout plan',
  place: { id: 'topic-1', kind: 'topic', name: 'Fix login', redacted: false },
  description: 'How we ship the redirect fix', updated_at: '2026-10-08T17:04:05.123Z', updated_by: person,
  part: { part: { block: BLOCK }, number: 3, anchor: BLOCK, text: 'Ship it behind the flag.', by: person },
};

const calls = vi.hoisted(() => ({
  preview: vi.fn<() => Promise<unknown>>(),
  document: vi.fn<() => Promise<unknown>>(),
  copy: vi.fn<(text: string, said?: string) => Promise<boolean>>(),
  filePreview: vi.fn<(url: string) => void>(),
}));
vi.mock('@/api/references', async (original) => {
  const actual = await original<typeof References>();
  return { ...actual, referencePreviewQuery: (reference: Parameters<typeof actual.referencePreviewQuery>[0]) => ({ ...actual.referencePreviewQuery(reference), queryFn: calls.preview }) };
});
vi.mock('@/api/documents', async (original) => {
  const actual = await original<typeof Documents>();
  return { ...actual, documentQuery: (id: string) => ({ ...actual.documentQuery(id), queryFn: calls.document }) };
});
vi.mock('@/ui/clipboard', async (original) => ({ ...await original<typeof Clipboard>(), copyText: calls.copy }));
vi.mock('@/features/files/source', async (original) => {
  const actual = await original<typeof FileSources>();
  return { ...actual, previewQuery: (source: Parameters<typeof actual.previewQuery>[0]) => {
    calls.filePreview(source.url); return actual.previewQuery(source);
  } };
});
vi.mock('@/realtime/live',async original=>({...await original(),useSocketStatus:()=>({phase:'ready'})}));

beforeEach(() => {
  vi.clearAllMocks();
  calls.preview.mockResolvedValue(preview);
  calls.document.mockResolvedValue({ id: DOC, title: 'Rollout plan' });
  calls.copy.mockResolvedValue(true);
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

const chip = () => show(<NamedChip path={`/doc/${DOC}`} id={DOC} kind="doc" original={i18n.t('people.privateReference')} reference={{ kind: 'document', id: DOC, block: BLOCK }} />);
const card = () => document.querySelector('[data-reference-card]');

it('reads a Large reference only when it becomes visible and keeps that single answer', async () => {
  const callbacks: IntersectionObserverCallback[] = [];
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: IntersectionObserverCallback) { callbacks.push(callback); }
    observe = vi.fn(); disconnect = vi.fn(); unobserve = vi.fn();
  });
  show(<ReferenceCard reference={{ kind: 'document', id: DOC }} label="Rollout plan"
    presentation={{ place: 'document', document: true, size: 'large' }}>
    {(keep) => <button type="button" onClick={keep}>Rollout plan</button>}
  </ReferenceCard>);
  await act(async () => { await Promise.resolve(); });
  expect(calls.preview).not.toHaveBeenCalled();
  act(() => { for (const callback of callbacks) callback([{ isIntersecting: false } as IntersectionObserverEntry], {} as IntersectionObserver); });
  expect(calls.preview).not.toHaveBeenCalled();
  act(() => { for (const callback of callbacks) callback([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver); });
  expect(await screen.findByText('Rollout plan')).toBeInTheDocument();
  expect(calls.preview).toHaveBeenCalledTimes(1);
  act(() => { for (const callback of callbacks) callback([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver); });
  expect(calls.preview).toHaveBeenCalledTimes(1);
});

it.each(['small', 'medium', 'large'] as const)('keeps a private file label and request without current metadata or media reads at %s size', async (size) => {
  vi.stubGlobal('IntersectionObserver', class {
    callback: IntersectionObserverCallback;
    constructor(callback: IntersectionObserverCallback) { this.callback = callback; }
    observe = (target: Element) => queueMicrotask(() => this.callback([{ target, isIntersecting: true } as IntersectionObserverEntry], this as unknown as IntersectionObserver));
    disconnect = vi.fn(); unobserve = vi.fn();
  });
  calls.preview.mockResolvedValue({ ...preview, kind: 'environment_file', title: 'Current secret filename', description: 'Current secret description',
    place: { id: 'hidden-place', kind: 'topic', name: 'Current secret place', redacted: false },
    file: { path: 'private/current-file.bin', size: 123456, owner_kind: null, owner_id: null, owner_name: 'Current secret owner', missing: false, blob: 'a'.repeat(64), media_type: 'application/secret' },
    unavailable: { deleted: false, object: { kind: 'environment', id: DOC }, permission: 'files.view', requestable: true, pending: null } });
  show(<NamedChip path={`/environments/${DOC}/files/shared/file.bin`} id={DOC} kind="node" reference={{ kind: 'environment_file', id: DOC, path: 'shared/file.bin', label: 'Shared filename' }} presentation={{ place: 'prose', size }} />);
  if (size !== 'large') await userEvent.click(await screen.findByRole('button', { name: 'Shared filename' }));
  expect(await screen.findByText(i18n.t('people.reference.noAccess'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('access.request.send') })).toBeEnabled();
  for (const secret of ['Current secret filename', 'Current secret description', 'Current secret place', 'Current secret owner', 'private/current-file.bin', 'application/secret', 'Ship it behind the flag.', 'Ada', 'a'.repeat(64)]) expect(document.body.innerHTML).not.toContain(secret);
  expect(calls.filePreview).not.toHaveBeenCalled();
  expect(calls.document).not.toHaveBeenCalled();
  expect(calls.preview).toHaveBeenCalledOnce();
});

it('opens under the pointer with the thing, where it lies, what it is about, its last change and the block it names', async () => {
  const user = userEvent.setup();
  chip();
  const link = await screen.findByRole('link', { name: 'Rollout plan' });
  await user.hover(link);
  await waitFor(() => expect(card()).not.toBeNull(), { timeout: 2_000 });
  const shown = card();
  expect(shown).toHaveTextContent('Fix login');
  expect(shown).toHaveTextContent('How we ship the redirect fix');
  expect(shown).toHaveTextContent(i18n.t('people.reference.block', { number: 3 }));
  expect(shown).toHaveTextContent('Ship it behind the flag.');
  for (const action of [i18n.t('people.reference.open'), i18n.t('workbench.workspace.beside'), i18n.t('people.reference.copy')]) {
    expect(screen.getByRole('button', { name: action })).toBeInTheDocument();
  }
  expect(calls.preview).toHaveBeenCalledTimes(1);
});

it('opens under the keyboard focus, and Enter keeps it open with its actions', async () => {
  const user = userEvent.setup();
  chip();
  await screen.findByRole('link', { name: 'Rollout plan' });
  await user.tab();
  await waitFor(() => expect(card()).not.toBeNull(), { timeout: 2_000 });
  await user.keyboard('{Enter}');
  const copy = await screen.findByRole('button', { name: i18n.t('people.reference.copy') });
  await user.click(copy);
  expect(calls.copy).toHaveBeenCalledWith(`[Rollout plan](id:${DOC}#3)`, i18n.t('people.reference.copied'));
});

it('keeps a private reference focusable with its shared name and a metadata-free card', async () => {
  const user=userEvent.setup();
  calls.preview.mockResolvedValue({...preview,title:'',description:'',place:null,updated_by:null,updated_at:null,part:null,file:null,unavailable:{deleted:false,object:{kind:'document',id:DOC},permission:'doc.view',requestable:true,pending:null}});
  show(<NamedChip path={`/doc/${DOC}`} id={DOC} kind="doc" original="The shared title" reference={{kind:'document',id:DOC,label:'The shared title'}} />);
  const chip=await screen.findByRole('button',{name:'The shared title'});
  expect(chip).toHaveAttribute('data-reference-state','no-access');
  await user.click(chip);
  await screen.findByText(i18n.t('people.reference.noAccess'));
  await user.click(chip);
  expect(card()).toBeVisible();
  expect(card()).toHaveTextContent('The shared title');
  expect(card()).toHaveTextContent(i18n.t('people.reference.ownersAsked'));
  expect(screen.getByRole('button',{name:i18n.t('access.request.send')})).toBeEnabled();
  for(const secret of ['Rollout plan','Fix login','How we ship the redirect fix','Ship it behind the flag.','Ada'])expect(card()).not.toHaveTextContent(secret);
  expect(calls.preview).toHaveBeenCalledOnce();expect(calls.document).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');await waitFor(()=>expect(card()).toBeNull());
  expect(chip).toHaveFocus();
});

it('a private direct-chat card contains no request form or current metadata',async()=>{
  calls.preview.mockResolvedValue({...preview,kind:'chat',title:'',description:'',place:null,updated_by:null,updated_at:null,part:null,file:null,unavailable:{deleted:false,object:null,permission:null,requestable:false,pending:null}});
  show(<NamedChip path={`/chat/${DOC}`} id={DOC} kind="chat" original="A shared chat" />);
  await userEvent.click(await screen.findByRole('button',{name:'A shared chat'}));
  expect(await screen.findByText(i18n.t('people.reference.noAccess'))).toBeInTheDocument();
  expect(card()?.textContent).toBe(i18n.t('people.reference.noAccess'));
  expect(screen.queryByRole('textbox')).toBeNull();expect(screen.queryByRole('button',{name:i18n.t('access.request.send')})).toBeNull();
});

it('keeps the reference card and document intact while its access request date is chosen in the editor',async()=>{
 const approval:Extract<Approval,{kind:'access'}>={id:'grant',kind:'access',state:'pending',title:'Request for the plan',object:{kind:'document',id:DOC,name:'Rollout plan'},node:null,
  requester:{user:person},request:{permissions:['doc.view'],options:null,holder:{kind:'user',user:person},reason:'Working together',for_thread:null},
  required_permission:'access.manage',can_decide:true,approver_scope:null,batch_id:'batch',thread_id:null,item_id:null,created_at:new Date().toISOString(),expires_at:null,
  decided_at:null,decided_by:null,decision:null,decided_automatically:false,quick_grant_durations:['1d','7d','30d','always']};
 calls.preview.mockResolvedValue({...preview,access_requests:[approval]});
 const world=new World();const ada=world.people.get('ada');const stored=[...world.documents.docs.values()].find(value=>value.title==='Rollout plan');
 if(ada===undefined||stored===undefined)throw new Error('The readable document fixture is required.');
 const doc=world.documents.full(ada,stored);const first=world.documents.sync(ada,stored,null).blocks[0];
 if(first===undefined)throw new Error('The fixture must include a block.');
 const block={...first,parent_id:null,position:'1',content:[{type:'ref' as const,kind:'document' as const,id:DOC,label:'Shared label',display:'small' as const}]};
 const ready=vi.fn<(api:EditorApi|null)=>void>();
 show(<DocumentMarksProvider owner="reference-date" documentId={doc.id}><BlockEditor document={doc} editable fixed={[block]} inspection onReady={ready}/></DocumentMarksProvider>);
 const link=await screen.findByRole('link',{name:'Rollout plan'});const editor=ready.mock.calls.at(-1)?.[0]?.editor;
 if(editor===undefined)throw new Error('The editor must be ready.');
 const before=editor.getJSON();
 act(()=>link.focus());await userEvent.keyboard('{Enter}');
 await userEvent.click(screen.getByRole('button',{name:i18n.t('approvals.quick.other')}));
 await userEvent.click(screen.getByRole('combobox',{name:i18n.t('approvals.access.ends')}));
 await userEvent.click(await screen.findByRole('option',{name:i18n.t('approvals.access.customEnd')}));
 expect(screen.getByLabelText(i18n.t('approvals.access.endsOn'),{exact:true})).toBeInTheDocument();
 expect(card()).not.toBeNull();
 expect(link.isConnected).toBe(true);
 expect(editor.getJSON()).toEqual(before);
});

it('a private group card asks to join without revealing the current name or place',async()=>{
  calls.preview.mockResolvedValue({...preview,kind:'chat',title:'',description:'',place:null,updated_by:null,updated_at:null,part:null,file:null,unavailable:{deleted:false,object:{kind:'chat',id:DOC},permission:null,requestable:true,pending:null}});
  show(<NamedChip path={`/chat/${DOC}`} id={DOC} kind="chat" original="A shared group" />);
  await userEvent.click(await screen.findByRole('button',{name:'A shared group'}));
  expect(await screen.findByText(i18n.t('access.request.join'))).toBeInTheDocument();
  expect(screen.getByRole('textbox',{name:i18n.t('access.request.reason')+' '+i18n.t('ui.field.optional')})).toBeInTheDocument();
  expect(card()).not.toHaveTextContent('Rollout plan');expect(card()).not.toHaveTextContent('Fix login');
});

it('reads a reference from its address and writes the words an agent follows', () => {
  expect(referenceOfAddress(`/documents/${DOC}#block=${BLOCK}`)).toEqual({ kind: 'document', id: DOC, block: BLOCK });
  expect(referenceOfAddress(`/nodes/${DOC}#turn=4`)).toEqual({ kind: 'node', id: DOC, turn: 4 });
  expect(referenceOfAddress(`/chats/${DOC}#message=12`)).toEqual({ kind: 'chat', id: DOC, message: 12 });
  expect(referenceOfAddress('/doc/elsewhere')).toBeNull();
  expect(referenceTarget(preview)).toBe(`/doc/${DOC}?block=${BLOCK}`);
  expect(referenceText({ ...preview, title: 'Plan [v2]' })).toBe(`[Plan \\[v2\\]](id:${DOC}#3)`);
  expect(referenceText({ ...preview, part: null })).toBe(`[Rollout plan](id:${DOC})`);
});
