/** A search result names its thing with the shared card and preserves the result's ordinary navigation. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Hit } from '@/features/search/Hit';
import type { ReferencePreview, SearchHit } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { agentMessage } from '../doubles/thread-rows';
import { display } from '../doubles/thread-display';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';
const calls=vi.hoisted(()=>({preview:vi.fn(),go:vi.fn(),reference:vi.fn()}));
vi.mock('@tanstack/react-router',async(original)=>({...await original<Record<string,unknown>>(),useRouter:()=>({history:{push:calls.go}})}));
vi.mock('@/api/references',()=>({referencePreviewQuery:(reference:unknown)=>{calls.reference(reference);return {queryKey:['result-preview',reference],queryFn:calls.preview};}}));
const node={id:'0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e03',kind:'topic',name:'Rollout plan',redacted:false} as const;
const hit:SearchHit={kind:'name',node,path:[node],href:`/topic/${node.id}`,at:'2026-01-01T00:00:00Z',snippet:'Release checklist',matches:[[0,7]]};
const preview:ReferencePreview={kind:'node',id:node.id,node_kind:'topic',title:node.name,place:null,description:'Checks before switching traffic',updated_at:null,updated_by:null,part:null};
beforeEach(()=>{vi.clearAllMocks();calls.preview.mockResolvedValue(preview);});
afterEach(async()=>{cleanup();await setLocale('en');});
it.each(['keyboard','tap'] as const)('opens the existing card from the result chip by %s without navigating',async(mode)=>{
  const user=userEvent.setup();const view=show(<ul><Hit hit={hit} slugOfOrg={()=>null}/></ul>);
  const chip=screen.getByRole('link',{name:node.name});
  expect(chip.closest('a')?.parentElement?.closest('a')).toBeNull();
  expect(calls.preview).not.toHaveBeenCalled();
  if(mode==='keyboard'){await user.tab();expect(chip).toHaveFocus();await user.keyboard('{Enter}');}
  else await user.click(chip);
  await waitFor(()=>expect(document.querySelector('[data-reference-card]')).toHaveTextContent(preview.description));
  expect(calls.reference).toHaveBeenLastCalledWith({kind:'node',id:node.id});
  expect(screen.getByRole('button',{name:i18n.t('people.reference.open')})).toBeVisible();
  expect(calls.go).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  await waitFor(()=>expect(document.querySelector('[data-reference-card]')).toBeNull());
  await user.click(screen.getByRole('link',{name:'Release checklist'}));
  expect(calls.go).toHaveBeenCalledExactlyOnceWith(`/topic/${node.id}`);
  expect(view.container.querySelector('mark')).toHaveTextContent('Release');
});

it('marks a meaning-only result without replacing an unhighlighted words result with that marker',()=>{
  const meaning=show(<ul><Hit hit={{...hit,matches:[]}} foundBy="meaning" slugOfOrg={()=>null}/></ul>);
  expect(screen.getByText(i18n.t('search.similarMeaning'))).toHaveClass('text-text-muted');
  meaning.unmount();
  show(<ul><Hit hit={{...hit,matches:[]}} foundBy="words" slugOfOrg={()=>null}/></ul>);
  expect(screen.queryByText(i18n.t('search.similarMeaning'))).not.toBeInTheDocument();
  expect(screen.getByText(hit.snippet)).toBeVisible();
});

it.each(['waiting_approval', 'blocked', 'failed', 'running', 'queued', 'held', 'finished', 'idle', 'archived'] as const)(
  'keeps the server %s icon in both thread names and transcript results without reading the thread', (state) => {
    const thread = {...node, kind: 'thread' as const, thread_display: display(state)};
    const named: SearchHit = {...hit, node: thread, path: [thread], href: `/thread/${thread.id}`};
    const transcript: SearchHit = {kind: 'item', item: agentMessage('state-answer', 'Release checklist'), thread,
      topic: node, project: {...node, kind: 'project'}, snippet: hit.snippet, matches: hit.matches};
    for (const entry of [named, transcript]) {
      const view = show(<ul><Hit hit={entry} slugOfOrg={()=>null}/></ul>);
      const chip = screen.getByRole('link', {name: new RegExp(node.name)});
      expect(chip.querySelector(`[data-work-state="${state}"]`)).not.toBeNull();
      expect(chip.querySelector('[role="img"]')).toHaveAttribute('aria-label');
      expect(calls.preview).not.toHaveBeenCalled();
      expect(calls.go).not.toHaveBeenCalled();
      view.unmount();
    }
  },
);

it('keeps a known thread without readable state facts quiet', () => {
  const thread = {...node, kind: 'thread' as const};
  const view = show(<ul><Hit hit={{...hit, node: thread, path: [thread]}} slugOfOrg={()=>null}/></ul>);
  expect(view.container.querySelector('[data-work-state]')).toBeNull();
  expect(screen.getByRole('link', {name: node.name}).querySelector('svg.lucide-message-square-text')).not.toBeNull();
  expect(calls.preview).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('describes the matching text rather than an unnamed field in %s', async (locale) => {
  await setLocale(locale);
  const document = {id: 'result-document', title: 'Release document', description: '', description_source: 'user', description_line: '',
    node, owner: ADA, owners: 1, created_by: ADA, page: 'follows', seq: 1, block_count: 1, bytes: 10, files_bytes: 0,
    version: 1, open_discussions: 0, updated_at: hit.at, updated_by: ADA, discussion_level: 'involved', seen_seq: null, archived: false, deleted: false} as const;
  const thread = {...node, kind: 'thread'} as const;
  const described = {...hit, match_field: 'description' as const};
  const cases: {value: SearchHit; en: string; zh: string}[] = [
    {value: hit, en: 'In the name', zh: '在名称中'},
    {value: described, en: 'In the description', zh: '在描述中'},
    {value: {kind: 'document', document, block_id: null, by: ADA, path: [node], snippet: 'Release', matches: [[0,7]]}, en: 'In the document title', zh: '在文档标题中'},
    {value: {kind: 'document', document, block_id: 'block', by: ADA, path: [node], snippet: 'Release', matches: [[0,7]]}, en: 'In the document', zh: '在文档中'},
    {value: {kind: 'item', item: agentMessage('answer', 'Release'), thread, topic: node, project: {...node,kind:'project'}, snippet: 'Release', matches: [[0,7]]}, en: 'In what the agent answered', zh: '在智能体的回答中'},
    {value: {kind: 'chat_message', chat: {id: 'chat',kind:'node',node,name:null,org_id:'org'}, path: [node],
      message: {id: 'message',chat_id:'chat',kind:'message',author:ADA,created_at:hit.at,edited_at:null,deleted:false,parts:[{type:'text',text:'Release'}],reactions:[],mentions:[],mentions_all:false,mention_targets:[],asked_agents:[],pinned:null,reply_to:null,seq_created:1,seq_updated:1,system:null}, snippet:'Release',matches:[[0,7]]}, en: 'In chat messages', zh: '在聊天消息中'},
  ];
  for (const entry of cases) {
    const view = show(<ul><Hit hit={entry.value} slugOfOrg={()=>null}/></ul>);
    expect(screen.getByText(locale === 'en' ? entry.en : entry.zh, {exact:true})).toBeVisible();
    expect(view.container.querySelector('mark')).toHaveTextContent('Release');
    view.unmount();
  }
  for (const [field, expected] of [['document_heading', locale === 'en' ? 'In the document heading' : '在文档小标题中'], ['file_name', locale === 'en' ? 'In the file name' : '在文件名中']] as const) {
    const value: SearchHit = {kind:'document',document,block_id:field === 'file_name' ? null : 'heading',by:ADA,path:[node],snippet:'Release',matches:[[0,7]]};
    const view = show(<ul><Hit hit={value} matchField={field} slugOfOrg={()=>null}/></ul>);
    expect(screen.getByText(expected, {exact:true})).toBeVisible();
    view.unmount();
  }
});
