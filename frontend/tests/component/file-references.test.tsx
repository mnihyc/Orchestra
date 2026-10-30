/** File references keep their environment and line, show a readable card, and copy words an agent can follow. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useQueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as References from '@/api/references';
import type * as Share from '@/api/share';
import type * as Workspace from '@/api/workspace';
import { AgentReferences } from '@/features/people/AgentReferences';
import { TypedChip } from '@/features/people/NamedChip';
import { attachmentFileReference, environmentFileReference, idReferenceOf, referenceOfAddress, referenceTarget, referenceText, type TypedReference } from '@/features/people/references';
import { useFileActions } from '@/features/files/FileMenu';
import { FileLink, workspacePath } from '@/features/files/FileLink';
import type { EnvironmentWorkspaceAccess } from '@/features/files/door';
import type { DirEntry, ReferencePreview, ReferencePreviewFile } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { Markdown } from '@/ui/Markdown';
import { ORCH_REFERENCE } from '@/lib/drop';
import { show } from './support';

const ENV = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e10';
const FILE = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e11';
const fileInfo = {path:'web/src/auth.rs',size:512,owner_kind:null,owner_id:null,owner_name:'Build environment',missing:false} satisfies ReferencePreviewFile;
const preview: ReferencePreview = {
  kind:'environment_file', id:ENV, node_kind:null, title:'auth.rs', place:null, description:'',
  updated_at:null, updated_by:null,
  part:{part:{line:42},number:42,anchor:null,text:'fn authenticate() {}',by:null},
  file:fileInfo,
};
const calls = vi.hoisted(() => ({ preview:vi.fn<(reference:TypedReference) => Promise<ReferencePreview>>(), copy:vi.fn(), open:vi.fn(), mayView:true, cwd:'/workspace/web', environment:{workspace_root:'/workspace',generation:1,facts:{}}, sharedEntries:[] as {id:string;name:string;path:string;kind:string;redacted:boolean;readable:boolean}[] }));
vi.mock('@/api/references', async (original) => {
  const actual = await original<typeof References>();
  return {...actual,referencePreviewQuery:(reference:TypedReference) => ({...actual.referencePreviewQuery(reference),queryFn:() => calls.preview(reference)})};
});
vi.mock('@/api/share', async (original) => {
  const actual = await original<typeof Share>();
  return {...actual,sharedTreeQuery:(thread:string,path='/') => ({...actual.sharedTreeQuery(thread,path),queryFn:() => Promise.resolve({entries:calls.sharedEntries})})};
});
vi.mock('@/api/workspace', async (original) => {
  const actual=await original<typeof Workspace>();
  return {...actual,environmentQuery:(id:string) => ({...actual.environmentQuery(id),queryFn:() => Promise.resolve(calls.environment)})};
});
vi.mock('@/realtime/stores/thread', async (original) => ({...await original(),useThreadRow:() => ({cwd:calls.cwd,directory:'web',environment:{id:ENV},environment_permissions:calls.mayView?['files.view']:[]})}));
vi.mock('@/features/files/FileTabs', async (original) => ({...await original(),openFile:calls.open}));
vi.mock('@/ui/clipboard', () => ({copyText:calls.copy}));

beforeEach(() => { vi.clearAllMocks(); calls.mayView=true; calls.cwd='/workspace/web'; calls.environment={workspace_root:'/workspace',generation:1,facts:{}}; calls.sharedEntries=[]; calls.preview.mockResolvedValue(preview); });
afterEach(() => cleanup());

function RefreshingFileReference() {
  const queries=useQueryClient();
  return <><TypedChip reference={{kind:'environment_file',id:ENV,path:'web/src/auth.rs',line:42}} /><button type="button" onClick={() => void queries.invalidateQueries({queryKey:['reference-preview']})}>Refresh</button></>;
}

it('shows a file chip with its environment, size and line and copies the same reference grammar', async () => {
  const user=userEvent.setup();
  show(<TypedChip reference={{kind:'environment_file',id:ENV,path:'web/src/auth.rs',line:42}} />);
  const chip=await screen.findByRole('link',{name:'auth.rs'});
  await user.click(chip);
  const owner=await screen.findByText('Build environment');
  expect(owner.closest('[data-reference-file]')).toBe(screen.getByText('web/src/auth.rs').closest('[data-reference-file]'));
  expect(screen.getByText(i18n.t('people.reference.line',{number:42}))).toBeInTheDocument();
  expect(screen.getByText('fn authenticate() {}')).toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:i18n.t('people.reference.copy')}));
  expect(calls.copy).toHaveBeenCalledWith(`[auth.rs](id:${ENV}/web/src/auth.rs#42)`,i18n.t('people.reference.copied'));
});

it('keeps unreadable files as locked kind-only chips without a legacy label', async () => {
  calls.preview.mockRejectedValue(new Error('not found'));
  show(<TypedChip reference={{kind:'file',id:FILE,line:2}} />);
  await waitFor(() => expect(calls.preview).toHaveBeenCalled());
  expect(screen.getByRole('button',{name:i18n.t('people.reference.kinds.file')})).toHaveAttribute('data-reference-state','no-access');
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
  expect(screen.queryByText('auth.rs')).not.toBeInTheDocument();
});

it('drags the shared file chip as its exact reference rather than an attachment copy', async () => {
  show(<TypedChip reference={{kind:'environment_file',id:ENV,path:'web/src/auth.rs',line:42}} />);
  const chip=await screen.findByRole('link',{name:'auth.rs'});
  expect(chip).toHaveAttribute('draggable','true');
  const transfer={setData:vi.fn(),effectAllowed:'none'};
  fireEvent.dragStart(chip,{dataTransfer:transfer});
  expect(transfer.setData).toHaveBeenCalledWith(ORCH_REFERENCE,`[auth.rs](id:${ENV}/web/src/auth.rs#42)`);
  expect(transfer.effectAllowed).toBe('copy');
});

it('hides a previously readable file name when its access check is refused again', async () => {
  const user=userEvent.setup();
  show(<RefreshingFileReference />);
  await screen.findByRole('link',{name:'auth.rs'});
  calls.preview.mockRejectedValue(new Error('not found'));
  await user.click(screen.getByRole('button',{name:'Refresh'}));
  await waitFor(() => expect(calls.preview).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.queryByRole('link',{name:'auth.rs'})).not.toBeInTheDocument());
  expect(screen.getByRole('button',{name:i18n.t('people.reference.kinds.environment_file')})).toHaveAttribute('data-reference-state','no-access');
});

it('keeps the same file chip while its card is being refreshed', async () => {
  const user=userEvent.setup();
  show(<RefreshingFileReference />);
  const chip=await screen.findByRole('link',{name:'auth.rs'});
  let resolve:((value:ReferencePreview) => void)|undefined;
  const pending=new Promise<ReferencePreview>((finish) => { resolve=finish; });
  calls.preview.mockReturnValueOnce(pending);
  await user.click(screen.getByRole('button',{name:'Refresh'}));
  await waitFor(() => expect(calls.preview).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('link',{name:'auth.rs'})).toBe(chip);
  const finish=resolve;
  if(finish===undefined) throw new Error('No pending preview');
  await act(async () => { finish(preview); await pending; });
  expect(screen.getByRole('link',{name:'auth.rs'})).toBe(chip);
});

it('shows Deleted without file metadata when the path is known gone', async () => {
  const user=userEvent.setup();
  calls.preview.mockResolvedValue({...preview,file:{...fileInfo,missing:true,path:'private/current.rs',owner_name:'Hidden current owner'}});
  show(<TypedChip reference={{kind:'environment_file',id:ENV,path:'web/src/auth.rs',line:42}} />);
  await user.click(await screen.findByRole('button',{name:i18n.t('people.reference.deleted')}));
  const card=document.querySelector('[data-reference-card]');
  expect(card).toHaveTextContent(i18n.t('people.reference.deleted'));
  expect(screen.queryByText('auth.rs')).not.toBeInTheDocument();
  expect(screen.queryByText('Build environment')).not.toBeInTheDocument();
  expect(screen.getByRole('button',{name:i18n.t('people.reference.open')})).toBeDisabled();
  expect(screen.getByRole('button',{name:i18n.t('workbench.workspace.beside')})).toBeDisabled();
  expect(screen.getByRole('button',{name:i18n.t('people.reference.copy')})).toBeEnabled();
  expect(card).not.toHaveTextContent('private/current.rs');
  expect(card).not.toHaveTextContent('Hidden current owner');
  await user.click(screen.getByRole('button',{name:i18n.t('people.reference.copy')}));
  expect(calls.copy).toHaveBeenCalledWith(`[Deleted](id:${ENV}/web/src/auth.rs#42)`,i18n.t('people.reference.copied'));
  expect(calls.open).not.toHaveBeenCalled();
});

it('shows an uncounted file size as a dash in the same metadata line', async () => {
  const user=userEvent.setup();
  calls.preview.mockResolvedValue({...preview,part:null,file:{...fileInfo,size:null}});
  show(<TypedChip reference={{kind:'environment_file',id:ENV,path:'web/src/auth.rs'}} />);
  await user.click(await screen.findByRole('link',{name:'auth.rs'}));
  const owner=await screen.findByText('Build environment');
  expect(owner.closest('[data-reference-file]')).toHaveTextContent('Build environment · web/src/auth.rs · –');
});

it('opens backquoted workspace paths in prose at the named line', async () => {
  const user=userEvent.setup();
  const content=<AgentReferences threadId="writer"><Markdown text="Look in `src/auth.rs:42`." /></AgentReferences>;
  show(content);
  const link=await screen.findByRole('button',{name:'src/auth.rs:42'});
  await user.click(link);
  expect(calls.open).toHaveBeenCalledWith({environmentId:ENV,threadId:'writer',path:'web/src/auth.rs',line:42,permanent:false});
});

it('leaves a prose file link plain without files access', () => {
  calls.mayView=false;
  show(<FileLink threadId="writer" path="src/auth.rs" line={42}>src/auth.rs:42</FileLink>);
  expect(screen.queryByRole('button',{name:'src/auth.rs:42'})).not.toBeInTheDocument();
  expect(screen.getByText('src/auth.rs:42')).toBeInTheDocument();
});

it('opens a colon line as a workspace file when a shared document has the same shorter path', async () => {
  const user=userEvent.setup();
  calls.sharedEntries=[{id:FILE,name:'Rollout plan',path:'docs/Rollout plan.md',kind:'document',redacted:false,readable:true}];
  show(<AgentReferences threadId="writer"><Markdown text="Read `docs/Rollout plan.md:2`." /></AgentReferences>);
  await user.click(await screen.findByRole('button',{name:'docs/Rollout plan.md:2'}));
  expect(calls.open).toHaveBeenCalledWith({environmentId:ENV,threadId:'writer',path:'web/docs/Rollout plan.md',line:2,permanent:false});
});

it('uses the actual working directory inside the environment when a thread starts further down', async () => {
  const user=userEvent.setup();
  calls.cwd='/workspace/web/api';
  show(<AgentReferences threadId="writer"><Markdown text="Read `src/auth.rs:42`." /></AgentReferences>);
  await user.click(await screen.findByRole('button',{name:'src/auth.rs:42'}));
  expect(calls.open).toHaveBeenCalledWith({environmentId:ENV,threadId:'writer',path:'web/api/src/auth.rs',line:42,permanent:false});
  expect(workspacePath('/workspace/shared/a.rs','/workspace/web/api','web','/workspace')).toBe('shared/a.rs');
  expect(workspacePath('C:\\work\\web\\a.rs','C:\\work\\web','web','C:\\work')).toBe('web/a.rs');
});

it('uses the current machine workspace spelling for relative and absolute prose paths', async () => {
  const user=userEvent.setup();
  calls.cwd='/registered/web/api';
  calls.environment={workspace_root:'/registered',generation:7,facts:{generation:7,workspace_root:'/native'}};
  show(<AgentReferences threadId="writer"><Markdown text="Read `src/auth.rs:2` and `/native/web/api/src/auth.rs:2`." /></AgentReferences>);
  await user.click(await screen.findByRole('button',{name:'src/auth.rs:2'}));
  await user.click(await screen.findByRole('button',{name:'/native/web/api/src/auth.rs:2'}));
  expect(calls.open).toHaveBeenCalledTimes(2);
  for (const [opened] of calls.open.mock.calls) expect(opened).toEqual({environmentId:ENV,threadId:'writer',path:'web/api/src/auth.rs',line:2,permanent:false});
});

it('does not use a workspace spelling reported by an earlier environment generation', async () => {
  const user=userEvent.setup();
  calls.cwd='/registered/web';
  calls.environment={workspace_root:'/registered',generation:7,facts:{generation:6,workspace_root:'/old'}};
  show(<AgentReferences threadId="writer"><Markdown text="Read `/registered/web/src/auth.rs:2`." /></AgentReferences>);
  await user.click(await screen.findByRole('button',{name:'/registered/web/src/auth.rs:2'}));
  expect(calls.open).toHaveBeenCalledWith({environmentId:ENV,threadId:'writer',path:'web/src/auth.rs',line:2,permanent:false});
});

it('round trips stored file addresses and copied paths with spaces and a selection first line', () => {
  expect(referenceOfAddress(`/files/${FILE}#line=2`)).toEqual({kind:'file',id:FILE,line:2});
  expect(referenceOfAddress(`/environments/${ENV}/files/web/a%20b.rs#line=42`)).toEqual({kind:'environment_file',id:ENV,path:'web/a b.rs',line:42});
  expect(idReferenceOf(`id:${ENV}/web/src/auth.rs#42`)).toEqual({id:ENV,path:'web/src/auth.rs',number:42});
  expect(environmentFileReference(ENV,'web/a b.rs',7)).toBe(`[a b.rs:7](<id:${ENV}/web/a b.rs#7>)`);
  expect(attachmentFileReference(FILE,'report.txt',7)).toBe(`[report.txt:7](id:${FILE}#7)`);
  expect(referenceTarget(preview)).toContain('line=42');
  expect(referenceText(preview)).toBe(`[auth.rs](id:${ENV}/web/src/auth.rs#42)`);
});

it('copies a reference from the file panel without needing file management', async () => {
  const user=userEvent.setup();
  const entry:DirEntry={name:'auth.rs',path:'web/src/auth.rs',type:'file',size:512,mtime:'2026-10-24T00:00:00Z',mode:0o644,link_target:null,cached:false};
  const access:EnvironmentWorkspaceAccess={door:{environmentId:ENV,threadId:null},thread:null,environment:{id:ENV,name:'Build environment',ports_exposed:false,address:'team.build',placement:'managed',status:'running',owner:{id:'ada',kind:'human',username:'ada',display_name:'Ada',avatar:{text:'A',color:1}}},directory:'web',mayView:true,mayManage:false,mayShell:false,mayWatchProcesses:false};
  function Panel() {
    const actions=useFileActions({access,environment:undefined,open:vi.fn(),collapseAll:vi.fn(),search:vi.fn(),entries:() => [entry]});
    const copy=actions.entriesFor(entry,new Set(),[entry]).find((one) => one.id==='copy-reference');
    return <button type="button" onClick={() => { if(copy!==undefined && 'onSelect' in copy) copy.onSelect(); }}>Copy reference</button>;
  }
  show(<Panel />);
  await user.click(screen.getByRole('button',{name:'Copy reference'}));
  expect(calls.copy).toHaveBeenCalledWith(`[auth.rs](id:${ENV}/web/src/auth.rs)`,i18n.t('people.reference.copied'));
});

it('shows a disabled environment beside its file and the existing route to enable it',async()=>{
 calls.preview.mockResolvedValue({...preview,file:{...fileInfo,disabled:true,enable_path:`/resources/environments/${ENV}?tab=danger`}});
 show(<TypedChip reference={{kind:'environment_file',id:ENV,path:'web/src/auth.rs',line:42}}/>);
 const chip=await screen.findByRole('link',{name:`auth.rs ${i18n.t('environments.status.disabled')}`});await userEvent.click(chip);
 expect(await screen.findByRole('button',{name:i18n.t('people.reference.enableEnvironment')})).toBeEnabled();
 expect(document.querySelector('[data-reference-card]')).toHaveTextContent(i18n.t('environments.status.disabled'));
});
