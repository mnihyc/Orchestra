/** The block editor: Orch's own schema of twenty block types on a rich-text base, changes sent as operations on blocks, and no Save.
 *
 * It is an editor and nothing more: what a document is, on the wire and at rest, is Orch's blocks, and
 * the editor's state is turned into them and back at the edge of this component (`schema.ts`), so that
 * the editor can be replaced without touching a document. Typing is the editor's own; what is typed is
 * sent behind the person's back (`sync.ts`) and what others write is laid over the page (`remote.ts`).
 */
import { bindEditorMarks, useDocumentMarksStore } from '@/features/documents/marks';
import { Extension, type Editor } from '@tiptap/core';
import { Plugin, Selection } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';
import { Dropcursor, Gapcursor, UndoRedo } from '@tiptap/extensions';
import { EditorContent, useEditor } from '@tiptap/react';
import { useInfiniteQuery } from '@tanstack/react-query';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { X } from 'lucide-react';

import { InlineAlert } from '@/ui/InlineAlert';

import { convertText, documentsOfNodeQuery } from '@/api/documents';
import { documentReadersQuery } from '@/api/people';
import { queryClient } from '@/api/query';
import { buildTree } from '@/features/documents/blocks';
import { AddressDialog, askAddress, attachDocumentUpload, insertDocumentFile } from '@/features/documents/Media';
import { AttachButton, UploadChips } from '@/features/composer/Attachments';
import { DropTarget } from '@/features/composer/DropTarget';
import { carriesFiles, carriesReference, droppedReference } from '@/lib/drop';
import { useFileDrops } from '@/features/composer/FileDrops';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useUploads } from '@/lib/useUploads';
import {
  backspaceAtStart,
  containerAt,
  deleteAtEnd,
  deleteSelectedBlock,
  isTextContent,
  liftBlock,
  quickAction,
  sinkBlock,
  splitBlock,
  type Command,
} from '@/features/documents/commands';
import { Habits, SuggestMenu, Suggestions, sources } from '@/features/documents/BlockMenu';
import { Hint } from '@/features/help/Hint';
import { CodeColours } from '@/features/documents/codeColours';
import { columnViews, KeepColumns } from '@/features/documents/Columns';
import { drawnViews } from '@/features/documents/Drawn';
import { BlockContextEntries, BlockHandle } from '@/features/documents/Handle';
import { lineViews } from '@/features/documents/lines';
import { LinkCard, openLinkOnPress } from '@/features/documents/LinkCard';
import { PasteMarkdown, readAndPaste, type PasteNotice } from '@/features/documents/paste';
import { mediaViews } from '@/features/documents/Media';
import { newId, schemaExtensions, treeToJson } from '@/features/documents/schema';
import { createSync, type Sync } from '@/features/documents/sync';
import { tableViews, TableTools } from '@/features/documents/Tables';
import { commentOnSelection, TextBar, linkFieldOf } from '@/features/documents/TextBar';
import { togetherExtension } from '@/features/documents/Together';
import type { Document, DocumentBlock } from '@/generated/api-types';
import { documentStateNow, useDocumentState } from '@/realtime/stores/document';
import { useOrgTree } from '@/realtime/stores/tree';
import { ContextMenu } from '@/ui/ContextMenu';

import './editor.css';

/** How many blocks a page has before it is laid out where it is looked at, and not all at once. */
const LARGE_PAGE = 300;

/** The keys of a block editor, which come before the editor's own. */
const Keys = Extension.create({
  name: 'orchKeys',
  priority: 1000,
  addKeyboardShortcuts() {
    const run =
      (...commands: Command[]) =>
      () =>
        commands.some((command) => command(this.editor.state, this.editor.view.dispatch));
    const inside = (name: string) => {
      const { $from } = this.editor.state.selection;
      for (let depth = $from.depth; depth > 0; depth -= 1) if ($from.node(depth).type.name === name) return true;
      return false;
    };
    return {
      Enter: () => {
        const parent = this.editor.state.selection.$from.parent;
        if (isTextContent(parent)) return splitBlock(this.editor.state, this.editor.view.dispatch);
        // A cell, a caption: a new line inside it. Code takes its own Enter.
        if (parent.type.name !== 'codeBlock' && parent.inlineContent && !this.editor.state.selection.$from.parent.type.spec.code) {
          return this.editor.commands.setHardBreak();
        }
        return false;
      },
      Backspace: run(backspaceAtStart, deleteSelectedBlock),
      Delete: run(deleteAtEnd, deleteSelectedBlock),
      Tab: () => {
        if (inside('table')) return false;
        if (this.editor.state.selection.$from.parent.type.name === 'codeBlock') return this.editor.commands.insertContent('  ');
        // The key stays in the page: a block that cannot go deeper stays where it is.
        sinkBlock(this.editor.state, this.editor.view.dispatch);
        return true;
      },
      'Shift-Tab': () => {
        if (inside('table')) return false;
        liftBlock(this.editor.state, this.editor.view.dispatch);
        return true;
      },
      'Mod-Enter': run(quickAction),
      'Mod-k': () => {
        linkFieldOf(this.editor).setState({ open: true });
        return !this.editor.state.selection.empty;
      },
      'Mod-Shift-m': () => commentOnSelection(this.editor),
    };
  },
});

/** The line the caret is in says what it is for, while it is empty. */
const EmptyLine = (text: (name: string, level: number) => string | null) =>
  Extension.create({
    name: 'orchEmptyLine',
    addProseMirrorPlugins() {
      return [
        new Plugin({
          props: {
            decorations: (state) => {
              const { selection } = state;
              // A page with nothing on it says so in its one line, focused or not: that is where to begin.
              const only = state.doc.childCount === 1 ? state.doc.child(0) : null;
              if (only !== null && this.editor.isEditable && only.type.name === 'block' && only.childCount === 1 && only.child(0).type.name === 'paragraph' && only.child(0).content.size === 0) {
                const label = text('paragraph', 1);
                if (label !== null) return DecorationSet.create(state.doc, [Decoration.node(1, 1 + only.child(0).nodeSize, { class: 'is-empty-line', 'data-placeholder': label })]);
              }
              if (!selection.empty || !this.editor.isEditable || !this.editor.isFocused) return DecorationSet.empty;
              const parent = selection.$from.parent;
              if (parent.content.size !== 0 || !(isTextContent(parent) || parent.type.name === 'codeBlock')) return DecorationSet.empty;
              const label = text(parent.type.name, Number(parent.attrs.level ?? 1));
              if (label === null) return DecorationSet.empty;
              const start = selection.$from.before();
              return DecorationSet.create(state.doc, [Decoration.node(start, start + parent.nodeSize, { class: 'is-empty-line', 'data-placeholder': label })]);
            },
          },
        }),
      ];
    },
  });

/** What the editor tells the page that holds it. */
export interface EditorApi {
  editor: Editor;
  sync: Sync | null;
}

export interface BlockEditorProps {
  document: Document;
  /** Whether the reader may write in it: the document's `doc.edit`, and a place that is not archived. */
  editable: boolean;
  /** A stored deleted page has no live collaboration, suggestions or mutation keys. */
  inspection?: boolean;
  /** The blocks of an earlier version, to be shown as they were. Such a page is only read: what others write now is not laid over it, and nothing is sent. */
  fixed?: readonly DocumentBlock[];
  onReady?: (api: EditorApi | null) => void;
}

/** A page of blocks to read and to write in. It waits for the document's blocks, then keeps them. */
export function BlockEditor(props: BlockEditorProps) {
  const state = useDocumentState(props.document.id);
  const { fixed } = props;
  if (fixed === undefined && !state.loaded) return null;
  return <LoadedEditor {...props} blocks={fixed?.length ?? state.blocks.size} />;
}

function LoadedEditor({ document: doc, editable, fixed, inspection = false, onReady, blocks }: BlockEditorProps & { blocks: number }) {
  const marks = useDocumentMarksStore();
  const { t } = useTranslation();
  const [pasteNotice, setPasteNotice] = useState<PasteNotice | null>(null);

  const extensions = useMemo(
    () => [
      ...schemaExtensions({ ...lineViews, ...tableViews, ...mediaViews, ...drawnViews, ...columnViews }),
      CodeColours,
      Extension.create({ name: 'orchDoc', addStorage: () => ({ id: doc.id, inspection }) }),
      UndoRedo,
      Dropcursor.configure({ width: 2, class: 'ProseMirror-dropcursor' }),
      Gapcursor,
      Suggestions.configure({people:async (query, cursor) => await queryClient.query({...documentReadersQuery(doc.id,query,cursor),staleTime:0})}),
      Habits,
      PasteMarkdown.configure({ report: setPasteNotice, convert: (format, text) => convertText(doc.id, format, text) }),
      ...(inspection ? [] : [Keys]),
      KeepColumns,
      EmptyLine((name, level) =>
        name === 'paragraph'
          ? t('documents.placeholder.paragraph')
          : name === 'heading'
            ? t('documents.placeholder.heading', { level })
            : name === 'bullet' || name === 'numbered'
              ? t('documents.placeholder.list')
              : name === 'todo'
                ? t('documents.placeholder.todo')
                : name === 'toggle'
                  ? t('documents.placeholder.toggle')
                  : name === 'quote'
                    ? t('documents.placeholder.quote')
                    : null,
      ),
      ...(inspection ? [] : [togetherExtension({ docId: doc.id, marks })]),
    ],
    // The editor is made once for a document; its words are those of the language it was opened in.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [doc.id],
  );

  const editor = useEditor(
    {
      extensions,
      editable,
      immediatelyRender: false,
      shouldRerenderOnTransaction: false,
      // Activity can retire and recreate an editor. Read confirmed blocks for each new instance,
      // not for each render or transaction; live changes still arrive through the block sync.
      onBeforeCreate: ({ editor: created }) => {
        created.setOptions({ content: treeToJson(buildTree(fixed ?? documentStateNow(doc.id).blocks.values())) });
      },
      editorProps: {
        attributes: {
          class: `doc-editor${editable ? '' : ' is-readonly'}`,
          'data-content-role': 'document',
          'aria-label': t('documents.page.body'),
          'aria-multiline': 'true',
          role: 'textbox',
          ...(blocks > LARGE_PAGE ? { 'data-large': 'true' } : {}),
        },
        // Ctrl+Click (⌘-click) on a link opens it in a new tab; so does any press of a reader who cannot write here.
        handleClick: (view, pos, event) => openLinkOnPress(view, pos, event),
      },
    },
    [doc.id],
  );

  if (editor === null) return null;
  return <ReadyEditor document={doc} editable={editable} fixed={fixed} inspection={inspection} onReady={onReady}
    editor={editor} pasteNotice={pasteNotice} reportPaste={setPasteNotice} dismissPaste={() => setPasteNotice(null)} />;
}

/** Controls and effects belong to a live editor instance, including when a retained pane wakes. */
function ReadyEditor({ document: doc, editable, fixed, inspection, onReady, editor, pasteNotice, reportPaste, dismissPaste }: BlockEditorProps & {
  editor: Editor; pasteNotice: PasteNotice | null; reportPaste: (notice: PasteNotice | null) => void; dismissPaste: () => void;
}) {
  const shown = useViewShown();
  const marks = useDocumentMarksStore();
  const { t } = useTranslation();
  useEffect(() => bindEditorMarks(editor, marks), [editor, marks]);

  const fileInput = useRef<HTMLInputElement>(null);
  const fileDrops = useFileDrops(`document:${doc.id}`);
  const uploads = useUploads(doc.id, (blob, signal) => attachDocumentUpload(doc.id, blob, signal), (file) => { insertDocumentFile(editor, file); });
  const addFiles = uploads.add;
  const putFiles = useCallback((files: Iterable<File>): void => {
    if (!editor.isEditable) return;
    const at = containerAt(editor.state.doc, editor.state.selection.from);
    let anchor = typeof at?.node.attrs.id === 'string' ? at.node.attrs.id : undefined;
    addFiles(files, (file) => { anchor = insertDocumentFile(editor, file, anchor) ?? anchor; });
  }, [editor, addFiles]);

  useEffect(() => {
    editor.setOptions({ editorProps: { ...editor.options.editorProps,
      handlePaste: (view, event) => {
        const files = event.clipboardData?.files;
        if (!view.editable || files === undefined || files.length === 0) return false;
        event.preventDefault(); putFiles(files); return true;
      },
      handleDrop: (view, event) => {
        if (!view.editable || event.dataTransfer === null || !carriesFiles(event.dataTransfer) && !carriesReference(event.dataTransfer)) return false;
        // The containing target captures the pointer before starting an asynchronous upload.
        event.preventDefault(); return true;
      },
    } });
  }, [editor, putFiles]);

  // Whether the reader may write follows what they hold, also when it changes under them.
  useEffect(() => {
    if (editor.isEditable === editable) return;
    editor.setEditable(editable);
  }, [editor, editable]);

  useEffect(() => {
    // An earlier version is shown as it was: it is not tied to the document as it is written now.
    if (fixed !== undefined) {
      onReady?.({ editor, sync: null });
      return () => onReady?.(null);
    }
    const made = createSync({ editor, docId: doc.id });
    onReady?.({ editor, sync: made });
    return () => {
      made.destroy();
      onReady?.(null);
    };
    // onReady is the page's, and changes with it; the editor is what this follows.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor, doc.id, fixed]);

  useSuggestSources(doc, editable && !inspection && shown);

  return (
    <div className="relative" data-document={doc.id}>
      {pasteNotice !== null && <InlineAlert tone={pasteNotice.error ? 'danger' : 'info'} className="mb-2"
        action={<button type="button" aria-label={t('common.dismiss')} onClick={dismissPaste} className="shrink-0 rounded-sm px-1"><X aria-hidden className="size-4" /></button>}>
        {pasteNotice.text}
      </InlineAlert>}
      {(editable || uploads.blocked || fileDrops.blocked) && <div className="mb-2 flex flex-col gap-2">
        <input ref={fileInput} type="file" multiple hidden data-document-files aria-label={t('uploads.choose')} onChange={(event) => { if (event.target.files !== null) putFiles(event.target.files); event.target.value = ''; }} />
        {editable && <span className="inline-flex items-center gap-1"><AttachButton help="documents" label={t('documents.files.attachTo', { name: doc.title || t('documents.page.untitled') })} onPress={() => { if (fileInput.current !== null) { fileInput.current.accept = ''; fileInput.current.click(); } }} /><Hint id="people-mentions" popover /></span>}
        <UploadChips queue={uploads} />
        {fileDrops.progress}
      </div>}
      {/* A right click inside a block gives the menu of that block, as its handle does. */}
      <DropTarget disabled={!editable || fixed !== undefined || inspection === true} label={t('uploads.target.document')} countLabel={(values) => t('uploads.target.documentCount', values)} onDrop={(data, point) => {
        if (!editor.isEditable || editor.isDestroyed) return;
        const at = editor.view.posAtCoords(point);
        if (at !== null) editor.view.dispatch(editor.state.tr.setSelection(Selection.near(editor.state.doc.resolve(at.pos))));
        const reference = droppedReference(data);
        if (reference !== null) void readAndPaste(editor.view, (format, text) => convertText(doc.id, format, text), reportPaste, 'markdown', reference, reference);
        else if (carriesFiles(data)) {
          // Capture a block id now: typing during the upload must not change its destination.
          const container = containerAt(editor.state.doc, editor.state.selection.from);
          let anchor = typeof container?.node.attrs.id === 'string' ? container.node.attrs.id : undefined;
          fileDrops.read(data, t('uploads.target.document'), (files) => addFiles(files, (file) => { anchor = insertDocumentFile(editor, file, anchor) ?? anchor; }));
        }
      }}>
      <ContextMenu label={t('documents.handle.menu')} disabled={!editable} content={<BlockContextEntries editor={editor} docId={doc.id} />}>
        <div>
          <EditorContent editor={editor} />
        </div>
      </ContextMenu>
      </DropTarget>
      {editable && <BelowTheLast editor={editor} />}
      {editable && <BlockHandle editor={editor} docId={doc.id} />}
      {editable && <TextBar editor={editor} docId={doc.id} />}
      {editable && <LinkCard editor={editor} editable={editable} />}
      {editable && <TableTools editor={editor} />}
      {!inspection && <SuggestMenu />}
      {!inspection && <AddressDialog />}
    </div>
  );
}

/** The ground under the last block: a press there puts the caret in a line after it, as a page offers. */
function BelowTheLast({ editor }: { editor: Editor }) {
  const { t } = useTranslation();
  return (
    <div
      className="h-16 cursor-text"
      aria-hidden
      title={t('documents.page.writeBelow')}
      onMouseDown={(event) => {
        event.preventDefault();
        const { doc } = editor.state;
        const last = doc.lastChild;
        const empty = last !== null && last.type.name === 'block' && last.childCount === 1 && last.child(0).type.name === 'paragraph' && last.child(0).content.size === 0;
        if (empty) {
          editor.commands.focus('end');
          return;
        }
        const paragraph = editor.schema.nodeFromJSON({ type: 'block', attrs: { id: newId() }, content: [{ type: 'paragraph' }] });
        const tr = editor.state.tr.insert(doc.content.size, paragraph);
        editor.view.dispatch(tr);
        editor.commands.focus('end');
      }}
    />
  );
}

/** Tells the lists that open at the caret whom and what a line can name. */
function useSuggestSources(doc: Document, enabled: boolean) {
  const root = doc.path[0]?.id ?? null;
  const tree = useOrgTree(root);
  const documents = useInfiniteQuery({ ...documentsOfNodeQuery(doc.node.id, { under: true }), enabled: enabled && !doc.node.redacted });
  useEffect(() => {
    if (!enabled) return;
    const nodes = [...tree.nodes.values()]
      .filter((node) => !node.deleted && node.kind !== 'org')
      .map((node) => ({ id: node.id, kind: 'node' as const, name: node.name, hint: node.kind, ...(node.kind === 'thread' ? { threadId: node.id } : {}) }));
    const written = (documents.data?.pages.flatMap((page) => page.items) ?? []).filter((one) => one.id !== doc.id).map((one) => ({ id: one.id, kind: 'document' as const, name: one.title, hint: 'document' }));
    sources.places = [...written, ...nodes];
  }, [enabled, tree.nodes, documents.data, doc.id]);
}

export { askAddress, containerAt };
