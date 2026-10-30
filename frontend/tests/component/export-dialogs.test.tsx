/** The small dialogs that give one thing as one file: what a thread, a chat and a document are asked for, how a number that is no turn is refused where it is typed, what a document's format could not say, and what a person who may not export is told. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type * as routerApi from '@tanstack/react-router';
import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type * as exportsApi from '@/api/exports';
import { exportChatFile, exportDocumentFile } from '@/api/exports';
import type * as historyApi from '@/api/threadHistory';
import { exportViewedThreadFile } from '@/api/threadHistory';
import { defaultThreadRead } from '@/realtime/threadScope';
import { ChatFile } from '@/features/export/ChatFile';
import { DocumentFile } from '@/features/export/DocumentFile';
import { saveFile } from '@/features/export/save';
import { ThreadFile } from '@/features/export/ThreadFile';
import type { Document, DocumentBlock } from '@/generated/api-types';
import { i18n } from '@/i18n';

import { show } from './support';

// A keyboard typed into a dialog takes long on a machine that is busy with other work.
vi.setConfig({ testTimeout: 30_000 });

const readDocument = vi.hoisted(() => ({ current: null as unknown as Document }));
/** The blocks of the document, as the dialog reads them where the document is not open in the page. */
const readBlocks = vi.hoisted(() => ({ current: [] as DocumentBlock[] }));
/** Where the page is, and the way the dialog goes elsewhere. */
const router = vi.hoisted(() => ({ pathname: '/doc/document-1', navigate: undefined as unknown as ReturnType<typeof vi.fn> }));

vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof routerApi>()),
  useNavigate: () => router.navigate,
  useRouterState: ({ select }: { select: (state: { location: { pathname: string } }) => unknown }) => select({ location: { pathname: router.pathname } }),
}));

vi.mock('@/api/exports', async (original) => ({
  ...(await original<typeof exportsApi>()),
  exportChatFile: vi.fn(),
  exportDocumentFile: vi.fn(),
  exportedDocumentQuery: (id: string) => ({ queryKey: ['doc', id], queryFn: () => Promise.resolve(readDocument.current) }),
  exportedBlocksQuery: (id: string) => ({ queryKey: ['blocks', id], queryFn: () => Promise.resolve(readBlocks.current) }),
}));
vi.mock('@/api/threadHistory', async (original) => ({ ...await original<typeof historyApi>(), exportViewedThreadFile: vi.fn() }));
vi.mock('@/features/export/save', () => ({ saveFile: vi.fn() }));

const FILE = { blob: new Blob(['x']), name: 'Main thread.md', losses: [] as string[] };

/** What a browser has and jsdom does not: the radio group measures itself. */
class Measures {
  observe = vi.fn();
  unobserve = vi.fn();
  disconnect = vi.fn();
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', Measures);
  vi.mocked(exportViewedThreadFile).mockReset().mockResolvedValue(FILE);
  vi.mocked(exportChatFile).mockReset().mockResolvedValue(FILE);
  vi.mocked(exportDocumentFile).mockReset().mockResolvedValue(FILE);
  vi.mocked(saveFile).mockReset();
  readBlocks.current = [];
  router.pathname = '/doc/document-1';
  router.navigate = vi.fn();
});

const target = (kind: 'thread' | 'chat' | 'document', name: string) => ({ kind, id: `${kind}-1`, name });

function draw(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return show(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

describe('a thread as one file', () => {
  it('asks for what was chosen: how much, the form, what only a person’s export adds, and the turns', async () => {
    const onClose = vi.fn();
    draw(<ThreadFile target={target('thread', 'Main thread')} open onClose={onClose} />);
    expect(screen.getByRole('dialog', { name: 'Export Main thread' })).toBeInTheDocument();

    const full = screen.getByRole('radio', { name: 'Full' });
    expect(full).toHaveAccessibleName('Full');
    expect(screen.getByText('Also what each step was given and what came back.')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('help.hints.export-detail.title') }) }));
    expect(await screen.findByRole('dialog', { name: i18n.t('help.hints.export-detail.title') })).toHaveTextContent('everything adds what each step was given and what came back');
    await userEvent.keyboard('{Escape}');
    await userEvent.click(full);
    await userEvent.click(screen.getByRole('checkbox', { name: /The agent.s reasoning/ }));
    await userEvent.click(screen.getByRole('checkbox', { name: /What sub-agents did/ }));
    await userEvent.type(screen.getByLabelText('From turn'), '2');
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));

    await waitFor(() => expect(exportViewedThreadFile).toHaveBeenCalled());
    const call = vi.mocked(exportViewedThreadFile).mock.calls[0];
    expect(call?.slice(0, 3)).toEqual(['thread-1', { format: 'markdown', detail: 'full', include: ['reasoning'], from: 2 }, undefined]);
    expect(call?.[3]?.ref).toEqual(defaultThreadRead('thread-1'));
    expect(call?.[3]?.threadId).toBe('thread-1');
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveFile).toHaveBeenCalledWith(FILE.blob, 'Main thread.md');
  });

  it('has what its sub-agents did in the file unless that is unticked, as the server does when it is not told', async () => {
    draw(<ThreadFile target={target('thread', 'Main thread')} open onClose={() => undefined} />);
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await waitFor(() => expect(exportViewedThreadFile).toHaveBeenCalled());
    expect(vi.mocked(exportViewedThreadFile).mock.calls[0]?.[1]).toMatchObject({ include: ['sub_streams'] });
  });

  it('refuses a turn that is none where it is typed, and a last turn before the first, and asks nothing', async () => {
    draw(<ThreadFile target={target('thread', 'Main thread')} open onClose={() => undefined} />);
    await userEvent.type(screen.getByLabelText('From turn'), '0');
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(screen.getByText(/“0” is not a turn/)).toBeInTheDocument();
    await userEvent.clear(screen.getByLabelText('From turn'));
    await userEvent.type(screen.getByLabelText('From turn'), '5');
    await userEvent.type(screen.getByLabelText('To turn'), '2');
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(screen.getByText('The last turn is before the first.')).toBeInTheDocument();
    expect(exportViewedThreadFile).not.toHaveBeenCalled();
  });

  it('says why when the file could not be made, and keeps the dialog', async () => {
    vi.mocked(exportViewedThreadFile).mockRejectedValue(new Error('the server said no'));
    draw(<ThreadFile target={target('thread', 'Main thread')} open onClose={() => undefined} />);
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(saveFile).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog', { name: 'Export Main thread' })).toBeInTheDocument();
  });
});

describe('a chat as one file', () => {
  it('asks for the form, the messages, and with the files an archive', async () => {
    draw(<ChatFile target={target('chat', 'Fix login')} open onClose={() => undefined} />);
    await userEvent.click(screen.getByRole('radio', { name: 'JSON' }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Attachments' }));
    await userEvent.type(screen.getByLabelText('To message'), '40');
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await waitFor(() => expect(exportChatFile).toHaveBeenCalled());
    expect(exportChatFile).toHaveBeenCalledWith('chat-1', { format: 'json', files: 'include', to: 40 });
  });
});

const owner = { id: 'ada', kind: 'human', username: 'ada', display_name: 'Ada Lovelace', avatar: { text: 'A', color: 1 } };
const textFile = { id: 'f1', name: 'plan.csv', media_type: 'text/csv', size: 40 * 1024, is_text: true, added_at: '2026-10-01T09:00:00.000Z', added_by: null };
const image = { id: 'f2', name: 'diagram.png', media_type: 'image/png', size: 90 * 1024, is_text: false, added_at: '2026-10-01T09:00:00.000Z', added_by: null };

/** A block of a document, as much of it as what a format loses is read from. */
function block(id: string, type: string, props: Record<string, unknown>, content: unknown[] = []): DocumentBlock {
  return { id, type, props, content, parent_id: null, removed: false, position: id } as unknown as DocumentBlock;
}

function document(more: Partial<Document> = {}, permissions = ['doc.view', 'doc.export']) {
  readDocument.current = { id: 'document-1', title: 'Rollout plan', owner, open_discussions: 2, files: [textFile, image], access: { administers: false, owns: false, permissions }, ...more } as unknown as Document;
}

describe('a document as one file', () => {
  it('asks for the format and the two things that are ticked, and counts the files that are no images', async () => {
    document();
    const onClose = vi.fn();
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={onClose} />);
    await screen.findByRole('checkbox', { name: 'Attachments' });
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('export.document.include') }) }));
    expect(within(screen.getByRole('dialog', { name: i18n.t('export.document.include') })).getByText('1 file (40 KiB); including attachments makes an archive (.zip).')).toBeVisible();
    expect(within(screen.getByRole('dialog', { name: i18n.t('export.document.include') })).getByText('2 open discussions, written after the text.')).toBeVisible();
    await userEvent.keyboard('{Escape}');

    await userEvent.click(screen.getByRole('radio', { name: /HTML/ }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Attachments' }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Comments' }));
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await waitFor(() => expect(exportDocumentFile).toHaveBeenCalled());
    expect(exportDocumentFile).toHaveBeenCalledWith('document-1', { format: 'html', attachments: 'include', comments: 'include' });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('has both ticks empty, the text and the images always, and Markdown chosen', async () => {
    document();
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={() => undefined} />);
    await screen.findByRole('checkbox', { name: 'Attachments' });
    expect(screen.getByRole('checkbox', { name: 'Text and images' })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Text and images' })).toBeDisabled();
    expect(screen.getByRole('checkbox', { name: 'Attachments' })).not.toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Comments' })).not.toBeChecked();
    expect(screen.getByRole('radio', { name: /Markdown/ })).toBeChecked();
  });

  it('leaves the attachments off, and says why, where the document holds no file but images', async () => {
    document({ files: [image] });
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={() => undefined} />);
    const attachments = await screen.findByRole('checkbox', { name: 'Attachments' });
    expect(attachments).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('export.document.include') }) }));
    expect(within(screen.getByRole('dialog', { name: i18n.t('export.document.include') })).getByText('No files besides images.')).toBeVisible();
    await userEvent.keyboard('{Escape}');
    expect(attachments).toBeDisabled();
  });

  it('says what the format could not say once the file is saved, and stays open to say it', async () => {
    document();
    const onClose = vi.fn();
    vi.mocked(exportDocumentFile).mockResolvedValue({ ...FILE, name: 'Rollout plan.md', losses: ['2 callouts become quotes', '1 pair of columns becomes one column'] });
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={onClose} />);
    await screen.findByRole('checkbox', { name: 'Attachments' });
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(await screen.findByText(/What this format could not say: 2 callouts become quotes, 1 pair of columns becomes one column\./)).toBeInTheDocument();
    expect(saveFile).toHaveBeenCalledWith(FILE.blob, 'Rollout plan.md');
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Export' })).not.toBeInTheDocument();
  });

  it('says what Markdown cannot say before the file is made, and says nothing of a format that keeps it all', async () => {
    document();
    readBlocks.current = [
      block('b1', 'callout', { tone: 'info' }),
      block('b2', 'paragraph', {}, [{ type: 'text', text: 'red', color: 1 }]),
      block('b3', 'columns', {}),
    ];
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={() => undefined} />);
    // Before anything is made: the dialog has asked nothing of the server.
    expect(await screen.findByText('Markdown cannot say:')).toBeInTheDocument();
    expect(screen.getByText('1 callout becomes a quote')).toBeInTheDocument();
    expect(screen.getByText('1 columns block is written one block after another')).toBeInTheDocument();
    expect(screen.getByText('colours are left out')).toBeInTheDocument();
    expect(exportDocumentFile).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole('radio', { name: /HTML/ }));
    expect(screen.queryByText('Markdown cannot say:')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('radio', { name: /Markdown/ }));
    expect(await screen.findByText('Markdown cannot say:')).toBeInTheDocument();
  });

  it('says nothing of what Markdown cannot say where the document says nothing that it cannot', async () => {
    document();
    readBlocks.current = [block('b1', 'paragraph', {}, [{ type: 'text', text: 'plain' }])];
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={() => undefined} />);
    await screen.findByRole('checkbox', { name: 'Attachments' });
    await waitFor(() => expect(screen.queryByText('Markdown cannot say:')).not.toBeInTheDocument());
  });

  it('offers a PDF that the browser makes: Print, where the document is open, which puts the dialog away and then prints the page', async () => {
    document();
    const print = vi.spyOn(window, 'print').mockImplementation(() => undefined);
    const onClose = vi.fn();
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={onClose} />);
    await screen.findByRole('checkbox', { name: 'Attachments' });
    await userEvent.click(screen.getByRole('radio', { name: /PDF/ }));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('export.document.as') }) }));
    expect(within(screen.getByRole('dialog', { name: i18n.t('export.document.as') })).getByText(/A PDF is made by your browser/)).toBeVisible();
    await userEvent.keyboard('{Escape}');
    // Nothing is included or left out of a page that is printed as it is.
    expect(screen.queryByRole('checkbox', { name: 'Attachments' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Export' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Print' }));
    expect(onClose).toHaveBeenCalled();
    await waitFor(() => expect(print).toHaveBeenCalledTimes(1));
    expect(exportDocumentFile).not.toHaveBeenCalled();
    print.mockRestore();
  });

  it('says that a PDF is printed from the page of the document where the document is not open, and offers to open it', async () => {
    document();
    router.pathname = '/org/acme';
    const print = vi.spyOn(window, 'print').mockImplementation(() => undefined);
    const onClose = vi.fn();
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={onClose} />);
    await screen.findByRole('checkbox', { name: 'Attachments' });
    await userEvent.click(screen.getByRole('radio', { name: /PDF/ }));
    expect(screen.getByText('A PDF is printed from the page of the document. Open it, and choose Export… there.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Print' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Open the document' }));
    expect(router.navigate).toHaveBeenCalledWith({ to: '/doc/$documentId', params: { documentId: 'document-1' } });
    expect(onClose).toHaveBeenCalled();
    expect(print).not.toHaveBeenCalled();
    print.mockRestore();
  });

  it('tells a person who reads it and may not export it for whom it is, and does not ask the server', async () => {
    document({}, ['doc.view']);
    draw(<DocumentFile target={target('document', 'Rollout plan')} open onClose={() => undefined} />);
    expect(await screen.findByText('Taking this away as a file is for people Ada Lovelace names')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(exportDocumentFile).not.toHaveBeenCalled();
  });
});
