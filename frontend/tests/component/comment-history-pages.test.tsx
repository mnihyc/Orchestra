/** A million-reply logical history replaces one window and keeps author actions across older pages. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, onTestFailed, vi } from 'vitest';
import { client } from '@/api/client';
import type { DocumentComment, DocumentDiscussion } from '@/api/documents';
import type * as Pages from '@/api/commentPages';
import type * as Jobs from '@/api/documentNoticeJobs';
import { CommentsPanel } from '@/features/documents/Comments';
import { DocumentMarksProvider, retireDocumentMarks } from '@/features/documents/marks';
import { retireCommentDraftSession } from '@/features/documents/commentDrafts';
import { documentStateNow, documentWriter } from '@/realtime/stores/document';
import { setLocale } from '@/i18n';
import { commentDocument } from './comment-document-fixture';
import { DORA } from '../doubles/chat-rows';
import { show } from './support';

// Shared identity, clock and reaction overlays have their own interaction suites.
// Keep this regression focused on the real comment rows, paging and author actions.
vi.mock('@/ui/UserChip',() => ({ UserChip:({user}:{user:{display_name:string}}) => <span>{user.display_name}</span> }));
vi.mock('@/ui/RelativeTime',() => ({ RelativeTime:({at}:{at:string}) => <time dateTime={at}>{at}</time> }));
vi.mock('@/ui/EmojiPicker',() => ({ ReactButton:() => <button type="button">React</button> }));
vi.mock('@/features/people/CommentField',() => ({ CommentField:({label,value,blockId}:{label:string;value:string;blockId?:string|null}) => <textarea readOnly aria-label={label} value={value} data-mention-block={blockId??''}/> }));

const calls=vi.hoisted(() => ({ read:vi.fn<(cursor:string|null) => Promise<Pages.CommentPage>>(),list:vi.fn<() => Promise<{items:DocumentDiscussion[];next_cursor:string|null}>>() }));
vi.mock('@/api/commentPages',async(load) => {
  const actual=await load<typeof Pages>();
  return { ...actual,
    commentPageQuery:(doc:string,discussion:string,cursor:string|null,seq:number) => ({ ...actual.commentPageQuery(doc,discussion,cursor,seq),queryFn:() => calls.read(cursor),retry:false }),
    discussionPageQuery:(...args:Parameters<typeof actual.discussionPageQuery>) => ({ ...actual.discussionPageQuery(...args),queryFn:() => calls.list(),retry:false }),
  };
});
vi.mock('@/api/documentNoticeJobs',async(load) => {
  const actual=await load<typeof Jobs>();
  return { ...actual,documentNoticeJobsQuery:(id:string) => ({ ...actual.documentNoticeJobsQuery(id),queryFn:() => Promise.resolve({items:[],next_cursor:null}) }) };
});
const original=client.session.getState();
beforeEach(async() => { await setLocale('en'); calls.read.mockReset(); calls.list.mockReset(); documentWriter.clear(); retireDocumentMarks(); retireCommentDraftSession(); localStorage.clear(); });
afterEach(() => { client.session.setState(original); documentWriter.clear(); retireDocumentMarks(); retireCommentDraftSession(); });

it('replaces comment pages, keeps author Resolve and returns to the latest replies',async() => {
  const started=performance.now(),phases:{name:string;ms:number}[]=[];
  const phase=(name:string) => phases.push({name,ms:Math.round(performance.now()-started)});
  onTestFailed(() => { console.info('Comment paging progress',phases,calls.read.mock.calls); });
  const full=structuredClone(commentDocument);
  const doc={ ...full,access:{ ...full.access,permissions:['doc.view','doc.comment'] } };
  const makeComment=(at:number):DocumentComment => ({ id:`reply-${at}`,discussion_id:'discussion',author:DORA,content:[{type:'text',text:`Historical reply ${at}`}],reactions:[],edited_at:null,deleted:false,created_at:'2026-01-01T00:00:00Z',seq_updated:at });
  const page=(cursor:string|null):Pages.CommentPage => {
    const after=cursor===null ? 1_000_000 : Number(cursor);
    return { items:Array.from({length:50},(_,i) => makeComment(after-49+i)),next_cursor:String(after-50),wrote:true };
  };
  calls.read.mockImplementation((cursor) => Promise.resolve(page(cursor)));
  const discussion:DocumentDiscussion={id:'discussion',document_id:doc.id,block_id:null,quote:'Long discussion',anchored:false,resolved:null,created_by:doc.owner,created_at:'2026-01-01T00:00:00Z',comments:page(null).items,seq_updated:1_000_000};
  documentWriter.reset(doc.id,{document:doc,discussions:[discussion]},1_000_000);
  client.session.setState({ status:'signed_in',userId:doc.owner.id,readerVersion:(original.readerVersion??0)+100 });
  const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  phase('fixture');
  show(<QueryClientProvider client={cache}><DocumentMarksProvider owner="comment-pages" documentId={doc.id}><CommentsPanel doc={doc}/></DocumentMarksProvider></QueryClientProvider>);
  phase('render');
  const history=within(screen.getByRole('group',{name:'Comment history'}));
  const older=await history.findByRole('button',{name:'Older comments'});
  await waitFor(() => expect(older).toBeEnabled());
  expect(screen.getByRole('button',{name:'Resolve'})).toBeVisible();
  phase('initial ready');
  for(let i=0;i<5;i+=1) {
    fireEvent.click(older);
    await waitFor(() => {
      expect(calls.read).toHaveBeenLastCalledWith(String(1_000_000-50*(i+1)));
      expect(document.querySelector(`[data-comment="reply-${1_000_000-50*(i+1)}"]`)).toHaveTextContent(`Historical reply ${1_000_000-50*(i+1)}`);
      expect(document.querySelectorAll('[data-comment]')).toHaveLength(50);
      expect(older).toBeEnabled();
    });
    phase(`older ${i+1}`);
  }
  expect(screen.queryByText('Historical reply 1000000')).not.toBeInTheDocument();
  expect(screen.getByText('Historical reply 999750')).toBeVisible();
  await waitFor(() => expect(cache.getQueryCache().findAll({queryKey:['document-comment-page']}).length).toBe(1));
  phase('cache');
  fireEvent.click(history.getByRole('button',{name:'Latest comments'}));
  expect(await screen.findByText('Historical reply 1000000')).toBeVisible();
  expect(document.querySelectorAll('[data-comment]')).toHaveLength(50);
  phase('latest');
});

it('keeps the mention anchor when a paged discussion is outside the live preview',async() => {
  const doc=structuredClone(commentDocument),block='00000000-0000-4000-8008-000000000002';
  const discussion:DocumentDiscussion={id:'old-discussion',document_id:doc.id,block_id:block,quote:'Older discussion',anchored:true,resolved:null,created_by:doc.owner,created_at:'2026-01-01T00:00:00Z',comments:[],seq_updated:1};
  documentWriter.reset(doc.id,{document:doc,discussions:Array.from({length:50},(_,i) => ({...discussion,id:`preview-${i}`}))},1);
  expect(documentStateNow(doc.id).discussions.has(discussion.id)).toBe(false);
  client.session.setState({status:'signed_in',userId:doc.owner.id,readerVersion:(original.readerVersion??0)+101});
  calls.list.mockResolvedValue({items:[discussion],next_cursor:null});
  const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  cache.setQueryData(['document-discussion-page',doc.id,'open',null,1],{items:[discussion],next_cursor:null});
  show(<QueryClientProvider client={cache}><DocumentMarksProvider owner="comment-anchor" documentId={doc.id}><CommentsPanel doc={doc}/></DocumentMarksProvider></QueryClientProvider>);
  const card=await screen.findByRole('region',{name:'Discussion'});
  expect(card).toHaveAttribute('data-discussion-card','old-discussion');
  expect(within(card).getByRole('textbox')).toHaveAttribute('data-mention-block',block);
});
