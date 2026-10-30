/** Images, films, sounds and files as blocks: put into the document, or for a film and a sound given as an address, which is played only once the reader chose to.
 *
 * A film or a sound given as an address shows the address with "Play from <host>": the reader's own
 * browser plays it from there once they pressed it, never before, so that opening a document tells no
 * other host who read it. What a document holds itself (an image, a data set) is read from the
 * system with the reader's own sign-in and shown from there. A file is a chip with its name and size.
 */
import { mergeAttributes, type AnyExtension, type Editor } from '@tiptap/core';
import { Selection } from '@tiptap/pm/state';
import { NodeViewContent, NodeViewWrapper, ReactNodeViewRenderer, type NodeViewProps } from '@tiptap/react';
import { Film, Music, Play } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { create } from 'zustand';

import { AudioBlock, blockToJson, FileBlock, ImageBlock, newId, VideoBlock } from '@/features/documents/schema';
import { containerAt } from '@/features/documents/commands';
import type { BlockType, DocumentFile, UploadedBlob } from '@/generated/api-types';
import { attachToDocument } from '@/lib/uploads';
import { documentStateNow, documentWriter, useDocumentState } from '@/realtime/stores/document';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { Input } from '@/ui/Input';
import { FileAttachment } from '@/features/files/FileChip';
import { FileState } from '@/features/files/FilePreview';
import { documentSource } from '@/features/files/source';
import { isChipSize } from '@/features/people/chipSizes';

/** The block that holds these bytes: recognized media are shown, and everything else is a file. */
export function fileBlockType(mediaType: string): Extract<BlockType, 'image' | 'video' | 'audio' | 'file'> {
  if (mediaType.startsWith('image/')) return 'image';
  if (mediaType.startsWith('video/')) return 'video';
  if (mediaType.startsWith('audio/')) return 'audio';
  return 'file';
}

/** Opens the editor's own picker from its block menu. */
export function askDocumentFiles(editor: Editor, images = false): void {
  const storage = editor.storage as { orchDoc?: { id: string } };
  const id = storage.orchDoc?.id;
  if (id === undefined || editor.isDestroyed || !editor.isEditable || !editor.view.dom.isConnected) return;
  const root = editor.view.dom.closest<HTMLElement>('[data-document]');
  const picker = root?.dataset.document === id ? root.querySelector<HTMLInputElement>('input[data-document-files]') : null;
  if (picker === null) return;
  picker.accept = images ? 'image/*' : '';
  picker.click();
}

/** Takes the returned file metadata at once; the act registry also refreshes the document's live reader. */
export async function attachDocumentUpload(id: string, uploaded: UploadedBlob, signal: AbortSignal): Promise<DocumentFile> {
  const file = await attachToDocument(id, uploaded, signal);
  const state = documentStateNow(id);
  if (state.document !== null) {
    const files = [...state.document.files.filter((held) => held.id !== file.id), file];
    documentWriter.apply(id, { document: { ...state.document, files, files_bytes: files.reduce((sum, held) => sum + held.size, 0) } }, state.lastSeq);
  }
  return file;
}

/** Inserts media after the block where it was chosen; a removed anchor puts it at the end. */
export function insertDocumentFile(editor: Editor, file: DocumentFile, anchor?: string): string | null {
  if (!editor.isEditable || editor.isDestroyed) return null;
  const id = newId();
  const block = editor.schema.nodeFromJSON(blockToJson({ id, type: fileBlockType(file.media_type), props: { file_id: file.id }, content: [] }));
  const { state } = editor;
  let at = anchor === undefined ? containerAt(state.doc, state.selection.from) : null;
  if (anchor !== undefined) state.doc.descendants((node, pos) => {
    if (node.attrs.id === anchor) { at = { node, pos, depth: 1, parent: state.doc, index: 0 }; return false; }
    return true;
  });
  const position = at === null ? state.doc.content.size : at.pos + at.node.nodeSize;
  const tr = state.tr.insert(position, block);
  const end = position + block.nodeSize;
  if (tr.doc.resolve(end).nodeAfter === null) tr.insert(end, state.schema.nodeFromJSON({ type: 'block', attrs: { id: newId() }, content: [{ type: 'paragraph' }] }));
  tr.setSelection(Selection.near(tr.doc.resolve(end), 1));
  editor.view.dispatch(tr.scrollIntoView());
  return id;
}

// ---------------------------------------------------------------- asking for an address

interface Asking {
  kind: 'video' | 'audio';
  done: (url: string) => void;
}

const useAsking = create<{ asking: Asking | null }>(() => ({ asking: null }));

/** Asks the person for the address of a film or a sound, and calls `done` with an `https` address. */
export function askAddress(kind: 'video' | 'audio', done: (url: string) => void): void {
  useAsking.setState({ asking: { kind, done } });
}

/** Whether an address may be given to a film or a sound: it is `https` and names a host. */
export function mediaAddress(text: string): URL | null {
  try {
    const url = new URL(text.trim());
    return url.protocol === 'https:' && url.hostname !== '' ? url : null;
  } catch {
    return null;
  }
}

/** What the field shows while it is empty. */
const ADDRESS_HINT = 'https://';

/** The dialog that asks for an address. It stands once for the page, and its form is made anew each time it opens. */
export function AddressDialog() {
  const asking = useAsking((state) => state.asking);
  return asking === null ? null : <AddressForm asking={asking} />;
}

function AddressForm({ asking }: { asking: Asking }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  const [wrong, setWrong] = useState(false);
  const close = () => useAsking.setState({ asking: null });
  const submit = () => {
    const url = mediaAddress(text);
    if (url === null) {
      setWrong(true);
      return;
    }
    asking.done(url.toString());
    close();
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && close()}
      title={t(asking.kind === 'audio' ? 'documents.media.soundTitle' : 'documents.media.filmTitle')}
      description={t('documents.media.addressAbout')}
      size="sm"
      footer={
        <>
          <Button variant="secondary" onClick={close}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" onClick={submit}>
            {t('documents.media.add')}
          </Button>
        </>
      }
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Input
          label={t('documents.media.address')}
          value={text}
          onChange={(event) => {
            setText(event.target.value);
            setWrong(false);
          }}
          placeholder={ADDRESS_HINT}
          error={wrong ? t('documents.media.addressWrong') : undefined}
          autoFocus
        />
      </form>
    </Dialog>
  );
}

// ---------------------------------------------------------------- what a document holds

function useDocumentId(props: NodeViewProps): string | null {
  const storage = props.editor.storage as { orchDoc?: { id: string } };
  return storage.orchDoc?.id ?? null;
}

const caption = (placeholder: string) => (
  <NodeViewContent
    as={'figcaption' as 'div'}
    // The caption is empty until somebody writes one, and says what it is for then.
    data-placeholder={placeholder}
  />
);

/** Stored media and file blocks share the same thumbnail, chip and viewer. */
function StoredFileView(props: NodeViewProps) {
  const { t } = useTranslation();
  const documentId = useDocumentId(props);
  const state = useDocumentState(documentId);
  const file = state.document?.files.find((one) => one.id === props.node.attrs.file_id);
  const inspect = (props.editor.storage as { orchDoc?: { inspection?: boolean } }).orchDoc?.inspection === true;
  return <NodeViewWrapper as="figure" className="oc oc-figure" data-ot={props.node.type.name}>
    <div className="oc-media" contentEditable={false}>
      {documentId === null || file === undefined ? <FileState state="gone" /> :
        <FileAttachment source={documentSource(documentId, file, inspect)} presentation={{ document: true, place: 'document',
          size: isChipSize(props.node.attrs.display) ? props.node.attrs.display : undefined,
          onSizeChange: props.editor.isEditable ? (display) => props.updateAttributes({ display }) : undefined }} />}
    </div>
    {caption(t('documents.media.caption'))}
  </NodeViewWrapper>;
}

/** A film or a sound: its address and a button, or its player once the reader pressed it. */
function PlayerView(props: NodeViewProps) {
  const { t } = useTranslation();
  const kind = props.node.type.name === 'audio' ? 'audio' : 'video';
  const address = typeof props.node.attrs.url === 'string' ? mediaAddress(props.node.attrs.url) : null;
  const [playing, setPlaying] = useState(false);
  const source = address !== null && playing ? address.toString() : null;
  const leaf = address?.pathname.split('/').filter(Boolean).at(-1);
  let filename = leaf;
  if (leaf !== undefined) { try { filename = decodeURIComponent(leaf); } catch { /* Keep a malformed escape readable as authored. */ } }
  const label = address === null ? '' : filename === undefined ? address.host : t('documents.media.hostFile', { host: address.host, name: filename });
  const Icon = kind === 'audio' ? Music : Film;
  if (address === null) return <StoredFileView {...props} />;
  return (
    <NodeViewWrapper as="figure" className="oc oc-figure" data-ot={kind}>
      <div className="oc-media" contentEditable={false}>
        {source !== null ? (
          kind === 'audio' ? (
            <audio controls src={source} preload="none" className="w-full" />
          ) : (
            <video controls src={source} preload="none" className="w-full" />
          )
        ) : (
          <div className="oc-box">
            <Icon aria-hidden className="size-4 text-text-muted" />
            <span className="min-w-0 flex-1 truncate text-sm text-text-muted" title={address.toString()}>{label}</span>
            <Button size="sm" className="shrink-0" icon={<Play aria-hidden />} onClick={() => setPlaying(true)}>
              {t(kind === 'audio' ? 'documents.media.playAudioFrom' : 'documents.media.playVideoFrom', { host: address.host })}
            </Button>
          </div>
        )}
      </div>
      {caption(t('documents.media.caption'))}
    </NodeViewWrapper>
  );
}

/** The nodes of media with the views that draw them. */
export const mediaViews: Readonly<Record<string, AnyExtension>> = {
  image: ImageBlock.extend({
    addNodeView: () => ReactNodeViewRenderer(StoredFileView),
    renderHTML: ({ HTMLAttributes }) => ['figure', mergeAttributes(HTMLAttributes, { class: 'oc oc-figure', 'data-ot': 'image' }), ['figcaption', 0]],
  }),
  video: VideoBlock.extend({ addNodeView: () => ReactNodeViewRenderer(PlayerView) }),
  audio: AudioBlock.extend({ addNodeView: () => ReactNodeViewRenderer(PlayerView) }),
  file: FileBlock.extend({ addNodeView: () => ReactNodeViewRenderer(StoredFileView) }),
};
