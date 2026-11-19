/** The handle beside a block: move it with what is under it, turn it into another type, remove it.
 *
 * "⋮⋮" at the left of a block shows on hover and on focus. Dragging it moves the block with everything
 * under it; clicking it opens the block's own menu: Turn into, Colour, Align, Duplicate, Comment, Copy
 * link to this block, Delete. A press does not open the menu, so that it can begin a drag. The same menu
 * opens by a right click anywhere in the block. The "+" beside it opens the block menu for a new block
 * under this one.
 */
import type { Editor } from '@tiptap/core';
import type { Node as PmNode } from '@tiptap/pm/model';
import type { NestedOptions } from '@tiptap/extension-drag-handle';
import type { TFunction } from 'i18next';
import { DragHandle } from '@tiptap/extension-drag-handle-react';
import { AlignCenter, AlignJustify, AlignLeft, AlignRight, GripVertical, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createStore, useStore, type StoreApi } from 'zustand';

import { blockOption, openMenuAfter } from '@/features/documents/BlockMenu';
import { duplicateBlock, removeBlock, turnInto } from '@/features/documents/commands';
import { marksOf } from '@/features/documents/marks';
import { TEXT_NODES } from '@/features/documents/schema';
import { tintEntries } from '@/features/documents/tints';
import { ContextEntries } from '@/ui/ContextMenu';
import { IconButton } from '@/ui/IconButton';
import { DropdownEntries, Menu, type MenuEntry } from '@/ui/Menu';
import { copyText } from '@/ui/clipboard';

const CONTAINERS = new Set(['block', 'leafBlock', 'columns']);

/**
 * Held outside the component and never rebuilt: the handle's plugin is taken out of the editor and put
 * back whenever one of these is a new object, and every plugin's views, the open block menu among them,
 * are made again each time that happens.
 */
const HANDLE_NESTING: NestedOptions = { defaultRules: false, edgeDetection: 'none', rules: [{ id: 'blocks', evaluate: ({ node }) => (CONTAINERS.has(node.type.name) ? 0 : 1000) }] };
const HANDLE_PLACEMENT = { placement: 'left-start' } as const;

/** Holds the handle where it is while its menu is open, and lets it follow the pointer again after. The handle's plugin reads this from a transaction's meta. */
function holdHandle(editor: Editor, held: boolean): void {
  editor.view.dispatch(editor.state.tr.setMeta('lockDragHandle', held).setMeta('addToHistory', false));
}

/** The ways a block's text can be set. */
const ALIGNS = [
  { value: 'left', icon: AlignLeft },
  { value: 'center', icon: AlignCenter },
  { value: 'right', icon: AlignRight },
  { value: 'justify', icon: AlignJustify },
] as const;

/** The types a block of text can be turned into. */
const TURN_INTO: readonly { id: string; node: string; attrs?: Record<string, unknown> }[] = [
  { id: 'paragraph', node: 'paragraph' },
  { id: 'heading1', node: 'heading', attrs: { level: 1 } },
  { id: 'heading2', node: 'heading', attrs: { level: 2 } },
  { id: 'heading3', node: 'heading', attrs: { level: 3 } },
  { id: 'heading4', node: 'heading', attrs: { level: 4 } },
  { id: 'heading5', node: 'heading', attrs: { level: 5 } },
  { id: 'heading6', node: 'heading', attrs: { level: 6 } },
  { id: 'bullet', node: 'bullet' },
  { id: 'numbered', node: 'numbered' },
  { id: 'todo', node: 'todo', attrs: { checked: false } },
  { id: 'toggle', node: 'toggle' },
  { id: 'quote', node: 'quote' },
  { id: 'callout', node: 'callout', attrs: { tone: 'info' } },
  { id: 'code', node: 'codeBlock', attrs: { language: '' } },
];

export interface Target {
  pos: number;
  node: PmNode;
}

/** The entries of the menu of the block at `target`. */
export function blockEntries(editor: Editor, target: Target, t: TFunction, docId: string): MenuEntry[] {
  const container = target.node;
  const content = container.type.name === 'columns' ? null : container.child(0);
  const text = content !== null && TEXT_NODES.includes(content.type.name);
  const holdsLook = content !== null && 'color' in (content.type.spec.attrs ?? {});
  const set = (name: string, value: unknown) => {
    editor.view.dispatch(editor.state.tr.setNodeAttribute(target.pos + 1, name, value));
  };
  const entries: MenuEntry[] = [];
  if (content !== null && (text || content.type.name === 'codeBlock')) {
    entries.push({
      type: 'sub',
      id: 'turn',
      label: t('documents.handle.turnInto'),
      entries: [
        {
          type: 'radio',
          id: 'turn-radio',
          label: t('documents.handle.turnInto'),
          value: TURN_INTO.find((one) => one.node === content.type.name && (one.attrs?.level === undefined || one.attrs.level === content.attrs.level))?.id ?? '',
          onValueChange: (id) => {
            const one = TURN_INTO.find((entry) => entry.id === id);
            if (one === undefined) return;
            const tr = editor.state.tr;
            if (turnInto(tr, target.pos, one.node, one.attrs ?? {})) editor.view.dispatch(tr);
          },
          options: TURN_INTO.map((one) => ({ value: one.id, ...blockOption(one.id) })),
        },
      ],
    });
  }
  if (holdsLook) {
    entries.push(
      { type: 'sub', id: 'color', label: t('documents.handle.color'), entries: tintEntries('color-radio', t('documents.handle.color'), (content.attrs.color as number | null | undefined) ?? null, (tint) => set('color', tint)) },
      { type: 'sub', id: 'background', label: t('documents.handle.background'), entries: tintEntries('background-radio', t('documents.handle.background'), (content.attrs.background as number | null | undefined) ?? null, (tint) => set('background', tint)) },
      {
        type: 'sub',
        id: 'align',
        label: t('documents.handle.align'),
        entries: [
          {
            type: 'radio',
            id: 'align-radio',
            label: t('documents.handle.align'),
            value: String(content.attrs.align ?? 'left'),
            onValueChange: (value) => set('align', value === 'left' ? null : value),
            options: ALIGNS.map((one) => ({ value: one.value, name: t(`choiceCatalog.alignment.${one.value}.name`), description: t(`choiceCatalog.alignment.${one.value}.description`), icon: <one.icon aria-hidden className="size-4" /> })),
          },
        ],
      },
    );
  }
  if (content?.type.name === 'heading') {
    entries.push({ type: 'check', id: 'folds', name: t('choiceCatalog.folding.on.name'), description: t('documents.handle.folds'), checked: content.attrs.toggle === true, onCheckedChange: (on) => set('toggle', on) });
  }
  if (entries.length > 0) entries.push({ type: 'separator', id: 'sep-look' });
  entries.push(
    {
      id: 'duplicate',
      label: t('documents.handle.duplicate'),
      onSelect: () => {
        const tr = duplicateBlock(editor.state, target.pos);
        if (tr !== null) editor.view.dispatch(tr);
      },
    },
    {
      id: 'comment',
      label: t('documents.handle.comment'),
      onSelect: () => marksOf(editor).setState({ compose: { block: String(container.attrs.id), quote: '' } }),
    },
    {
      id: 'link',
      label: t('documents.handle.copyLink'),
      onSelect: () => {
        const url = `${location.origin}/doc/${docId}?block=${String(container.attrs.id)}`;
        void copyText(url, t('common.linkCopied'));
      },
    },
    { type: 'separator', id: 'sep-delete' },
    {
      id: 'delete',
      label: t('documents.handle.delete'),
      danger: true,
      onSelect: () => {
        const tr = removeBlock(editor.state, target.pos);
        if (tr !== null) editor.view.dispatch(tr);
      },
    },
  );
  return entries;
}

/** The block the pointer is over, as the handle follows it: what a right click inside the page gives the menu of. */
const pointers = new WeakMap<Editor, StoreApi<{ target: Target | null }>>();
export function blockPointerOf(editor: Editor): StoreApi<{ target: Target | null }> {
  let pointer = pointers.get(editor);
  if (pointer === undefined) { pointer = createStore<{ target: Target | null }>(() => ({ target: null })); pointers.set(editor, pointer); }
  return pointer;
}

/** The handle that follows the pointer from block to block. */
export function BlockHandle({ editor, docId }: { editor: Editor; docId: string }) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<Target | null>(null);
  const [open, setOpen] = useState(false);
  const show = (next: boolean) => {
    setOpen(next);
    holdHandle(editor, next);
  };
  return (
    <DragHandle
      editor={editor}
      nested={HANDLE_NESTING}
      computePositionConfig={HANDLE_PLACEMENT}
      onNodeChange={({ node, pos }) => {
        const found = node === null || pos < 0 ? null : { node, pos };
        setTarget(found);
        blockPointerOf(editor).setState({ target: found });
      }}
      className="doc-handle"
    >
      <div className="flex items-center text-text-subtle" data-handle="">
        <IconButton size="sm" label={t('documents.handle.add')} icon={<Plus aria-hidden />} onClick={() => target !== null && openMenuAfter(editor, target.pos)} />
        <Menu
          label={t('documents.handle.menu')}
          side="bottom"
          open={open}
          onOpenChange={show}
          trigger={
            <IconButton
              size="sm"
              label={t('documents.handle.menu')}
              icon={<GripVertical aria-hidden />}
              className="cursor-grab active:cursor-grabbing"
              // A press on the handle begins a drag of its block: the menu opens on a click alone, and from the keys.
              onPointerDown={(event) => event.preventDefault()}
              onClick={() => show(true)}
            />
          }
          content={target === null ? null : <HandleEntries editor={editor} target={target} docId={docId} />}
        />
      </div>
    </DragHandle>
  );
}

function HandleEntries({ editor, target, docId }: { editor: Editor; target: Target; docId: string }) {
  const { t } = useTranslation();
  return <DropdownEntries entries={blockEntries(editor, target, t, docId)} />;
}

/** The menu of the block under the pointer, as a right click inside the page opens it: the same as its handle's. */
export function BlockContextEntries({ editor, docId }: { editor: Editor; docId: string }) {
  const { t } = useTranslation();
  const target = useStore(blockPointerOf(editor), (state) => state.target);
  if (target === null) return null;
  return <ContextEntries entries={blockEntries(editor, target, t, docId)} />;
}
