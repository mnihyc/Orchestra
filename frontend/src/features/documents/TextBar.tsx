/** The small bar over a selection of text: bold, italic, underline, strike-through, code, link, colour, highlight, Comment and Turn into.
 *
 * `Mod+B`, `Mod+I`, `Mod+U`, `Mod+E`, `Mod+K` and `Mod+Shift+M` do the same for the first six and for Comment.
 */
import type { Editor } from '@tiptap/core';
import type { Node as PmNode } from '@tiptap/pm/model';
import { NodeSelection, TextSelection } from '@tiptap/pm/state';
import { useEditorState } from '@tiptap/react';
import { BubbleMenu, type BubbleMenuProps } from '@tiptap/react/menus';
import { floatingClasses, floatingPlacement } from '@/ui/Popover';
import { Bold, Code, Highlighter, Italic, Link2, MessageSquarePlus, Palette, Strikethrough, Underline } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createStore, useStore, type StoreApi } from 'zustand';

import { linkAllowed } from '@/features/documents/schema';
import { marksOf } from '@/features/documents/marks';
import { blockEntries } from '@/features/documents/Handle';
import { tintEntries } from '@/features/documents/tints';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { DropdownEntries, Menu } from '@/ui/Menu';
import { Popover } from '@/ui/Popover';

/** Whether the field that takes a link is open: `Mod+K` opens it. */
const linkFields = new WeakMap<Editor, StoreApi<{ open: boolean }>>();
export function linkFieldOf(editor: Editor): StoreApi<{ open: boolean }> {
  let field = linkFields.get(editor);
  if (field === undefined) { field = createStore(() => ({ open: false })); linkFields.set(editor, field); }
  return field;
}

/** The words that are selected, or nothing. */
function selectedText(editor: Editor): string {
  const { from, to, empty } = editor.state.selection;
  return empty ? '' : editor.state.doc.textBetween(from, to, ' ', ' ').slice(0, 400);
}

/** Opens a discussion on the selected words, in the block they are in. */
export function commentOnSelection(editor: Editor): boolean {
  const { $from, empty } = editor.state.selection;
  if (empty) return false;
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    const node = $from.node(depth);
    if (node.type.name === 'block' || node.type.name === 'leafBlock') {
      marksOf(editor).setState({ compose: { block: String(node.attrs.id), quote: selectedText(editor) } });
      return true;
    }
  }
  return false;
}

/** What the field of a link shows while it is empty. */
const ADDRESS_HINT = 'https://';

/** The form that takes a link: made anew each time the field opens, with the address the selection links to now. */
function LinkForm({ editor, current }: { editor: Editor; current: string }) {
  const { t } = useTranslation();
  const [text, setText] = useState(current);
  const [wrong, setWrong] = useState(false);
  const apply = () => {
    if (text.trim() === '') {
      editor.chain().focus().unsetLink().run();
      linkFieldOf(editor).setState({ open: false });
      return;
    }
    if (!linkAllowed(text)) {
      setWrong(true);
      return;
    }
    editor.chain().focus().extendMarkRange('link').setLink({ href: text.trim() }).run();
    linkFieldOf(editor).setState({ open: false });
  };
  return (
    <form
      className="flex w-(--doc-menu) flex-col gap-2 p-1"
      onSubmit={(event) => {
        event.preventDefault();
        apply();
      }}
    >
      <Input
        label={t('documents.bar.address')}
        value={text}
        onChange={(event) => {
          setText(event.target.value);
          setWrong(false);
        }}
        placeholder={ADDRESS_HINT}
        error={wrong ? t('documents.bar.addressWrong') : undefined}
        autoFocus
      />
      <div className="flex justify-end gap-2">
        {current !== '' && (
          <Button
            type="button"
            variant="ghost"
            onClick={() => {
              editor.chain().focus().unsetLink().run();
              linkFieldOf(editor).setState({ open: false });
            }}
          >
            {t('documents.bar.unlink')}
          </Button>
        )}
        <Button type="submit" variant="primary">
          {t('documents.bar.apply')}
        </Button>
      </div>
    </form>
  );
}

function LinkField({ editor }: { editor: Editor }) {
  const { t } = useTranslation();
  const open = useStore(linkFieldOf(editor), (state) => state.open);
  const current = (editor.getAttributes('link').href as string | undefined) ?? '';
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        linkFieldOf(editor).setState({ open: next });
        if (!next) editor.commands.focus();
      }}
      label={t('documents.bar.link')}
      trigger={<IconButton label={t('documents.bar.link')} shortcut="Mod+K" pressed={editor.isActive('link')} icon={<Link2 aria-hidden />} />}
    >
      <LinkForm editor={editor} current={current} />
    </Popover>
  );
}

/** The marks the bar toggles, with the words and the key of each. */
const MARKS = [
  { name: 'bold', label: 'documents.bar.bold', shortcut: 'Mod+B', icon: Bold, toggle: 'toggleBold' },
  { name: 'italic', label: 'documents.bar.italic', shortcut: 'Mod+I', icon: Italic, toggle: 'toggleItalic' },
  { name: 'underline', label: 'documents.bar.underline', shortcut: 'Mod+U', icon: Underline, toggle: 'toggleUnderline' },
  { name: 'strike', label: 'documents.bar.strike', shortcut: 'Mod+Shift+S', icon: Strikethrough, toggle: 'toggleStrike' },
  { name: 'code', label: 'documents.bar.code', shortcut: 'Mod+E', icon: Code, toggle: 'toggleCode' },
] as const;

/** Whether the focus is in a bar or in a menu opened from it: the page is not focused then, and the bar stays. */
export function holdsFocus(bar: HTMLElement): boolean {
  const active = bar.ownerDocument.activeElement;
  return active !== null && (bar.contains(active) || active.closest('[role="menu"], [role="dialog"], [role="listbox"]') !== null);
}

/** The bar shows over a selection of words that can be written in. Held outside the component: a bar whose options are new on every render is reconfigured on every render. */
const showOverWords: NonNullable<BubbleMenuProps['shouldShow']> = ({ editor: current, element, view, state: pm }) =>
  current.isEditable &&
  (view.hasFocus() || holdsFocus(element)) &&
  !pm.selection.empty &&
  pm.selection instanceof TextSelection &&
  !(pm.selection instanceof NodeSelection) &&
  pm.selection.$from.parent.type.name !== 'codeBlock';
const BAR_PLACEMENT = { placement: 'top', offset: 8, ...floatingPlacement } as const;

/** The bar over the selection. It is there for whoever may write, and not for somebody who reads. */
export function TextBar({ editor, docId }: { editor: Editor; docId: string }) {
  const { t } = useTranslation();
  const state = useEditorState({
    editor,
    selector: ({ editor: current }) => ({
      bold: current.isActive('bold'),
      italic: current.isActive('italic'),
      underline: current.isActive('underline'),
      strike: current.isActive('strike'),
      code: current.isActive('code'),
      color: (current.getAttributes('textColor').tint as number | undefined) ?? null,
      background: (current.getAttributes('highlight').tint as number | undefined) ?? null,
    }),
  });
  const chain = () => editor.chain().focus();
  return (
    <BubbleMenu
      editor={editor}
      pluginKey="orchTextBar"
      shouldShow={showOverWords}
      options={BAR_PLACEMENT}
    >
      <div role="toolbar" aria-label={t('documents.bar.label')} className={cx('flex flex-wrap items-center gap-1 overflow-y-auto rounded-lg border border-border bg-surface-raised p-1 shadow-md', floatingClasses)}>
        {MARKS.map((one) => (
          <IconButton
            key={one.name}
            label={t(one.label)}
            shortcut={one.shortcut}
            pressed={state[one.name]}
            icon={<one.icon aria-hidden />}
            onClick={() => (chain() as unknown as Record<string, () => { run: () => boolean }>)[one.toggle]?.().run()}
          />
        ))}
        <LinkField editor={editor} />
        <Menu
          label={t('documents.bar.color')}
          trigger={<IconButton label={t('documents.bar.color')} icon={<Palette aria-hidden />} />}
          entries={tintEntries('text-color', t('documents.bar.color'), state.color, (tint) => (tint === null ? chain().unsetMark('textColor').run() : chain().setMark('textColor', { tint }).run()))}
        />
        <Menu
          label={t('documents.bar.highlight')}
          trigger={<IconButton label={t('documents.bar.highlight')} icon={<Highlighter aria-hidden />} />}
          entries={tintEntries('text-highlight', t('documents.bar.highlight'), state.background, (tint) => (tint === null ? chain().unsetMark('highlight').run() : chain().setMark('highlight', { tint }).run()))}
        />
        <IconButton label={t('documents.bar.comment')} shortcut="Mod+Shift+M" icon={<MessageSquarePlus aria-hidden />} onClick={() => commentOnSelection(editor)} />
        <Menu
          label={t('documents.handle.turnInto')}
          trigger={
            <Button size="sm" variant="ghost">
              {t('documents.handle.turnInto')}
            </Button>
          }
          content={<TurnIntoEntries editor={editor} docId={docId} />}
        />
      </div>
    </BubbleMenu>
  );
}

function TurnIntoEntries({ editor, docId }: { editor: Editor; docId: string }) {
  const { t } = useTranslation();
  const { from } = editor.state.selection;
  let found: { pos: number; node: PmNode } | null = null;
  const $from = editor.state.doc.resolve(from);
  for (let depth = $from.depth; depth > 0 && found === null; depth -= 1) {
    const node = $from.node(depth);
    if (node.type.name === 'block' || node.type.name === 'leafBlock') found = { pos: $from.before(depth), node };
  }
  if (found === null) return null;
  const entries = blockEntries(editor, found, t as never, docId).filter((entry) => entry.id === 'turn');
  const turn = entries[0];
  return turn?.type === 'sub' ? <DropdownEntries entries={turn.entries} /> : null;
}
