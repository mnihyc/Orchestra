/** File kinds share one lazy preview, one card and the normal viewer across people and agent surfaces. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { FileAttachment, FileChip } from '@/features/files/FileChip';
import { FilePreview } from '@/features/files/FilePreview';
import { blobSource, documentSource, sourceBytes, type FilePreview as Preview } from '@/features/files/source';
import { FileChip as ChatFileChip } from '@/features/chat/Attachments';
import { AttachmentChips } from '@/features/composer/Attachments';
import { MessageFiles } from '@/features/files/MessageFiles';
import { useWorkbench } from '@/features/workbench/store';
import { i18n } from '@/i18n';
import { ContextMenu } from '@/ui/ContextMenu';
import { show } from './support';

const calls = vi.hoisted(() => ({ request: vi.fn(), copy: vi.fn(), seen: [] as IntersectionObserverCallback[] }));
vi.mock('@/api/trace', async (original) => ({ ...await original(), traceRequest: calls.request }));
vi.mock('@/ui/clipboard', () => ({ copyText: calls.copy }));
vi.mock('@/features/files/viewers/Image', () => ({ default: () => <div data-viewer="image">Image viewer</div> }));
vi.mock('@/features/files/viewers/Pdf', () => ({ default: () => <div data-viewer="pdf">PDF viewer</div> }));
vi.mock('@/features/files/viewers/Media', () => ({ default: () => <div data-viewer="media">Media viewer</div> }));
vi.mock('@/features/files/viewers/Archive', () => ({ default: () => <div data-viewer="archive">Archive viewer</div> }));
vi.mock('@/features/files/viewers/Markdown', () => ({ default: () => <div data-viewer="markdown">Markdown viewer</div> }));
vi.mock('@/features/files/viewers/Code', () => ({ default: () => <div data-viewer="code">Code viewer</div> }));
vi.mock('@/features/files/viewers/NoPreview', () => ({ default: () => <div data-viewer="none">No preview viewer</div> }));
vi.mock('@/features/files/viewers/Difference', () => ({ default: () => <div data-viewer="difference">Difference viewer</div> }));

const HASH = 'a'.repeat(64);
const preview = (media = 'image/png', isText = false): Preview => ({ state: 'ready', hash: HASH, size: 128, media_type: media, is_text: isText,
  image: media.startsWith('image/') ? 'data:image/png;base64,cGl4ZWxz' : null, text: isText ? 'first line\nsecond line' : null,
  width: media.startsWith('image/') ? 64 : null, height: media.startsWith('image/') ? 32 : null, duration_seconds: null, entry_count: null });

function reveal() { act(() => { for (const callback of calls.seen) callback([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver); }); }

beforeEach(() => {
  calls.request.mockReset(); calls.copy.mockReset(); calls.seen = [];
  calls.request.mockImplementation(() => Promise.resolve(Response.json(preview())));
  useWorkbench.setState({ chipSizes: {} });
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: IntersectionObserverCallback) { calls.seen.push(callback); }
    observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn();
  });
});
afterEach(() => { vi.unstubAllGlobals(); });

it('requests no original or preview until a thumbnail is seen, then shares one request for the same source', async () => {
  const source = blobSource('chat', 'chat', HASH, 'photo.png');
  show(<><FileAttachment source={source} /><FileAttachment source={source} /></>);
  expect(calls.request).not.toHaveBeenCalled();
  reveal();
  await waitFor(() => expect(calls.request).toHaveBeenCalledTimes(1));
  expect(calls.request.mock.calls[0]?.[0]).toBe(`/chats/chat/blobs/${HASH}?preview=true`);
  expect(await screen.findAllByRole('img', { name: 'photo.png' })).toHaveLength(2);
  expect(calls.request.mock.calls.every(([url]) => String(url).includes('preview=true'))).toBe(true);
});

it('replaces a document thumbnail when the same file identifier gets a new version', async () => {
  const oldImage = preview().image;
  const newImage = 'data:image/png;base64,bmV3IHBpeGVscw==';
  calls.request.mockResolvedValueOnce(Response.json(preview())).mockResolvedValueOnce(Response.json({ ...preview(), hash: 'b'.repeat(64), image: newImage }));
  const file = { id: 'file', name: 'photo.png', media_type: 'image/png', size: 128, is_text: false, added_by: null, added_at: '2026-10-25T00:00:00Z' };
  const view = show(<FileAttachment source={documentSource('doc', file)} />);
  reveal();
  expect(await screen.findByRole('img', { name: 'photo.png' })).toHaveAttribute('src', oldImage);
  view.rerender(<FileAttachment source={documentSource('doc', { ...file, added_at: '2026-10-25T01:00:00Z' })} />);
  await waitFor(() => expect(screen.getByRole('img', { name: 'photo.png' })).toHaveAttribute('src', newImage));
  expect(calls.request).toHaveBeenCalledTimes(2);
});

const kinds = [
  ['photo.png', 'image/png', false, 'image'], ['photo.jpg', 'image/jpeg', false, 'image'],
  ['motion.gif', 'image/gif', false, 'image'], ['motion.webp', 'image/webp', false, 'image'],
  ['vector.svg', 'image/svg+xml', true, 'image'], ['phone.heic', 'image/heic', false, 'image'],
  ['report.pdf', 'application/pdf', false, 'pdf'], ['song.mp3', 'audio/mpeg', false, 'media'],
  ['film.mp4', 'video/mp4', false, 'media'], ['main.rs', 'text/plain', true, 'code'],
  ['readme.md', 'text/markdown', true, 'markdown'], ['changes.patch', 'text/plain', true, 'difference'],
  ['source.zip', 'application/zip', false, 'archive'], ['sheet.xlsx', 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', false, 'none'],
  ['data.bin', 'application/octet-stream', false, 'none'],
] as const;

describe.each(kinds)('%s', (name, media, isText, viewer) => {
  it('opens its existing workspace viewer from the same file card', async () => {
    const user = userEvent.setup();
    calls.request.mockImplementation(() => Promise.resolve(Response.json(preview(media, isText))));
    show(<FileAttachment source={blobSource('thread', 'thread', HASH, name, 128, media)} thumbnail={false} />);
    await user.click(screen.getByRole('button', { name }));
    reveal();
    await user.click(await screen.findByRole('button', { name: 'Open' }));
    await waitFor(() => expect(document.querySelector(`[data-file-overlay] [data-viewer="${viewer}"]`)).toBeInTheDocument());
    expect(screen.getByRole('dialog')).toHaveAccessibleName(name);
    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(screen.getByRole('button', { name })).toHaveFocus();
  });
});

describe.each(kinds)('%s across attachment surfaces', (name, media, isText, viewer) => {
  it.each(['chat', 'timeline', 'composer', 'document', 'search', 'inbox'] as const)('opens the common card and viewer in %s without eager reads', async (place) => {
    calls.request.mockImplementation(() => Promise.resolve(Response.json(preview(media, isText))));
    const source = blobSource('thread', 'thread', HASH, name, 128, media);
    const file = { id: 'file', name, media_type: media, size: 128, is_text: isText, added_by: null, added_at: '2026-10-25T00:00:00Z' };
    show(place === 'chat' ? <ChatFileChip chatId="chat" blob={HASH} name={name} size={128} mediaType={media} />
      : place === 'composer' ? <AttachmentChips parts={[{ type: 'attachment', blob: HASH, name, size: 128, media_type: media }]} onRemove={() => undefined} />
      : place === 'document' ? <FileAttachment source={documentSource('doc', file)} presentation={{ place: 'document', document: true }} />
      : place === 'search' || place === 'inbox' ? <MessageFiles files={[source]} />
      : <FileAttachment source={source} />);
    expect(calls.request).not.toHaveBeenCalled();
    if (place !== 'document') await userEvent.click(screen.getByRole('button', { name }));
    reveal();
    await waitFor(() => expect(document.querySelector('[data-file-card]')).toBeInTheDocument());
    expect(await screen.findByRole('button', { name: 'Open' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Download' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy reference' })).toBeInTheDocument();
    expect(calls.request.mock.calls.every(([url]) => String(url).includes('preview=true'))).toBe(true);
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));
    await waitFor(() => expect(document.querySelector(`[data-file-overlay] [data-viewer="${viewer}"]`)).toBeInTheDocument());
    expect(screen.getByRole('dialog')).toHaveAccessibleName(name);
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });
});

it.each(['unknown.constructor', 'unknown.__proto__'])('opens the ordinary binary viewer for the unknown suffix in %s', async (name) => {
  calls.request.mockImplementation(() => Promise.resolve(Response.json(preview('application/octet-stream'))));
  show(<FileAttachment source={blobSource('thread', 'thread', HASH, name)} />);
  await userEvent.click(screen.getByRole('button', { name }));
  reveal();
  await userEvent.click(await screen.findByRole('button', { name: 'Open' }));
  await waitFor(() => expect(document.querySelector('[data-file-overlay] [data-viewer="none"]')).toBeInTheDocument());
  expect(screen.getByRole('dialog')).toHaveAccessibleName(name);
});

it('uses the verified preview kind when a filename has no useful extension', async () => {
  show(<FileAttachment source={blobSource('thread', 'thread', HASH, 'artifact.bin')} />);
  expect(calls.request).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'artifact.bin' }));
  reveal();
  await waitFor(() => expect(document.querySelector('[data-file-card] img')).toHaveAttribute('alt', 'artifact.bin'));
  expect(calls.request).toHaveBeenCalledTimes(1);
  expect(calls.request.mock.calls.every(([url]) => String(url).includes('preview=true'))).toBe(true);
  await userEvent.click(screen.getByRole('button', { name: 'Open' }));
  await waitFor(() => expect(document.querySelector('[data-file-overlay] [data-viewer="image"]')).toBeInTheDocument());
});

it.each(['unavailable', 'too_large'] as const)('names the same %s state and retains the file actions', async (state) => {
  calls.request.mockImplementation(() => Promise.resolve(Response.json({ ...preview(), state, image: null })));
  show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'photo.png')} />);
  reveal();
  expect(await screen.findByText(i18n.t(state === 'too_large' ? 'fileMedia.tooLarge' : 'fileMedia.unavailable'))).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'photo.png' })).toBeInTheDocument();
});

it('does not keep an attachment name or card after a current read refuses permission', async () => {
  calls.request.mockRejectedValue(new ApiError({ status: 403, code: 'authz.denied', title: '', detail: '', type: '', request_id: '' }, null));
  show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'private.png')} />);
  reveal();
  expect(await screen.findByText(i18n.t('people.privateReference'))).toBeInTheDocument();
  expect(screen.queryByText('private.png')).not.toBeInTheDocument();
  expect(document.querySelector('[data-file-card]')).not.toBeInTheDocument();
});

it('keeps a gone original distinct from a broken decoder', async () => {
  calls.request.mockRejectedValue(new ApiError({ status: 404, code: 'not_found', title: '', detail: '', type: '', request_id: '' }, null));
  show(<FilePreview source={blobSource('chat', 'chat', HASH, 'gone.png')} />);
  reveal();
  expect(await screen.findByText('File no longer exists')).toBeInTheDocument();
});

it('names a gone file once in its card and disables its reads', async () => {
  calls.request.mockRejectedValue(new ApiError({ status: 404, code: 'not_found', title: '', detail: '', type: '', request_id: '' }, null));
  show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'gone.txt')} />);
  await userEvent.click(screen.getByRole('button', { name: 'gone.txt' }));
  reveal();
  expect(await screen.findAllByText('File no longer exists')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Open' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Download' })).toBeDisabled();
});

it('shows a ready archive count as its preview without an unavailable warning', async () => {
  calls.request.mockImplementation(() => Promise.resolve(Response.json({ ...preview('application/zip'), entry_count: 3 })));
  show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'source.zip', 128, 'application/zip')} />);
  await userEvent.click(screen.getByRole('button', { name: 'source.zip' }));
  reveal();
  expect(await screen.findByText('3 entries')).toBeInTheDocument();
  expect(screen.queryByText('Preview unavailable')).not.toBeInTheDocument();
});

it('sends an SVG preview only to an image element, never as markup', async () => {
  show(<FilePreview source={blobSource('chat', 'chat', HASH, 'vector.svg', 128, 'image/svg+xml')} />);
  reveal();
  const image = await screen.findByRole('img', { name: 'vector.svg' });
  expect(image).toHaveAttribute('src', expect.stringMatching(/^data:image\/png;base64,/));
  expect(image).toHaveStyle({ backgroundSize: '1rem 1rem' });
  expect(document.querySelector('svg script')).toBeNull();
});

it('reads only a bounded text prefix when a viewer asks for a range', async () => {
  calls.request.mockResolvedValue(new Response('hello', { status: 206, headers: { 'Content-Range': 'bytes 0-4/1000000' } }));
  const source = blobSource('chat', 'chat', HASH, 'large.txt');
  const result = await sourceBytes(source, HASH, 5);
  expect(result.total).toBe(1_000_000);
  expect(calls.request).toHaveBeenCalledWith(source.url, { headers: { Range: 'bytes=0-4' } });
});

it('copies the receiving path with its spaces and literal plus sign intact', async () => {
  calls.request.mockImplementation(() => Promise.resolve(Response.json(preview('text/plain', true), {
    headers: { 'x-orch-file-path': '.orch%2Fuploads%2Fa%2Bb+c.txt' },
  })));
  show(<FileAttachment source={blobSource('thread', 'thread', HASH, 'a+b c.txt', 128, 'text/plain')} />);
  await userEvent.click(screen.getByRole('button', { name: 'a+b c.txt' }));
  reveal();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Copy reference' })).not.toHaveAttribute('aria-disabled', 'true'));
  await userEvent.click(screen.getByRole('button', { name: 'Copy reference' }));
  expect(calls.copy).toHaveBeenCalledWith('[a+b c.txt](<.orch/uploads/a+b c.txt>)', i18n.t('people.reference.copied'));
});

it('keeps all three sizes in one family and uses Medium for a Large choice in a compact row', () => {
  useWorkbench.setState({ chipSizes: { compact: 'large', prose: 'medium' } });
  show(<><FileAttachment source={blobSource('chat', 'chat', HASH, 'small.txt', 128)} presentation={{ size: 'small' }} />
    <FileAttachment source={blobSource('chat', 'chat', HASH, 'medium.txt', 128)} />
    <FileAttachment source={blobSource('chat', 'chat', HASH, 'compact.txt', 128)} presentation={{ place: 'compact' }} /></>);
  expect(screen.getByRole('button', { name: 'small.txt' }).closest('[data-chip-size]')).toHaveAttribute('data-chip-size', 'small');
  expect(screen.getByRole('button', { name: 'medium.txt' }).closest('[data-chip-size]')).toHaveAttribute('data-chip-size', 'medium');
  expect(screen.getByRole('button', { name: 'compact.txt' }).closest('[data-chip-size]')).toHaveAttribute('data-chip-size', 'medium');
  expect(calls.request).not.toHaveBeenCalled();
});

it('offers Show as by the same context menu and writes the document choice instead of a reader preference', async () => {
  const change = vi.fn();
  show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'doc.txt', 128)} presentation={{ document: true, place: 'document', size: 'small', onSizeChange: change }} />);
  fireEvent.contextMenu(screen.getByRole('button', { name: 'doc.txt' }));
  await userEvent.hover(await screen.findByRole('menuitem', { name: 'Show as' }));
  await userEvent.click(await screen.findByRole('menuitemradio', { name: 'Large' }));
  expect(change).toHaveBeenCalledWith('large');
  expect(useWorkbench.getState().chipSizes).toEqual({});
});

describe.each(['compact', 'prose', 'document'] as const)('chip presentation in %s', (place) => {
  it.each(['small', 'medium', 'large'] as const)('renders %s with the common chip or in-flow card', (size) => {
    const shownSize = size === 'large' && place === 'compact' ? 'medium' : size;
    show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'notes.txt', 128, 'text/plain')}
      presentation={{ place, size, document: place === 'document' }} />);
    expect(document.querySelector('[data-chip-size]')).toHaveAttribute('data-chip-size', shownSize);
    expect(document.querySelector('[data-file-card]') !== null).toBe(shownSize === 'large');
    expect(screen.queryByText('128 B') !== null).toBe(shownSize !== 'small');
    expect(calls.request).not.toHaveBeenCalled();
  });
});

it.each([{ key: 'ContextMenu' }, { key: 'F10', shiftKey: true }])('opens Show as with $key and stores a reader choice', async (key) => {
  show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'notes.txt', 128)} presentation={{ place: 'compact' }} />);
  const trigger = screen.getByRole('button', { name: 'notes.txt' });
  trigger.focus();
  fireEvent.keyDown(trigger, key);
  await userEvent.hover(await screen.findByRole('menuitem', { name: 'Show as' }));
  await userEvent.click(await screen.findByRole('menuitemradio', { name: 'Large' }));
  expect(useWorkbench.getState().chipSizes).toEqual({ compact: 'large' });
  expect(trigger.closest('[data-chip-size]')).toHaveAttribute('data-chip-size', 'medium');
});

it('offers the stored sizes with an edit reason in a read-only document', async () => {
  show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'notes.txt', 128)} presentation={{ place: 'document', size: 'small', document: true }} />);
  fireEvent.contextMenu(screen.getByRole('button', { name: 'notes.txt' }));
  await userEvent.hover(await screen.findByRole('menuitem', { name: 'Show as' }));
  for (const name of ['Small', 'Medium', 'Large']) expect(await screen.findByRole('menuitem', { name: new RegExp(name) })).toHaveAttribute('aria-disabled', 'true');
  expect(useWorkbench.getState().chipSizes).toEqual({});
});

it('offers Show as on a touch long press', async () => {
  show(<FileAttachment source={blobSource('chat', 'chat', HASH, 'notes.txt', 128)} />);
  const trigger = screen.getByRole('button', { name: 'notes.txt' });
  fireEvent.pointerDown(trigger, { pointerType: 'touch', clientX: 20, clientY: 20 });
  expect(await screen.findByRole('menuitem', { name: 'Show as' })).toBeVisible();
  fireEvent.pointerUp(trigger, { pointerType: 'touch' });
  expect(calls.request).not.toHaveBeenCalled();
});

it('keeps a file row selection modifier and makes its native menu only when opened', async () => {
  const open = vi.fn(), action = vi.fn();
  const menu = vi.fn(() => [{ id: 'rename', label: 'Rename', onSelect: action }]);
  show(<ContextMenu label="Row actions" entries={[{ id: 'outer', label: 'Row actions', onSelect: () => undefined }]}>
    <div><FileChip source={blobSource('chat', 'chat', HASH, 'notes.txt', 128)} presentation={{ place: 'compact' }} onOpen={open} menuEntries={menu} /></div>
  </ContextMenu>);
  const trigger = screen.getByRole('button', { name: 'notes.txt' });
  fireEvent.click(trigger, { shiftKey: true });
  expect(open.mock.calls[0]?.[0]).toMatchObject({ shiftKey: true });
  expect(menu).not.toHaveBeenCalled();
  fireEvent.keyDown(trigger, { key: 'ContextMenu' });
  expect(await screen.findByRole('menuitem', { name: 'Show as' })).toBeVisible();
  expect(screen.queryByRole('menuitem', { name: 'Row actions' })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('menuitem', { name: 'Rename' }));
  expect(action).toHaveBeenCalledOnce();
  expect(calls.request).not.toHaveBeenCalled();
});
