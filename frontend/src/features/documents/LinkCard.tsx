/** The card of a link in a document, shown while the caret is in it: where it leads, and Open in new tab, Copy link and Edit; a Ctrl+Click (⌘-click) on a link opens it at once.
 *
 * A link in a page that is being written is text first: a press puts the caret in it, and the card under
 * it says where it leads and offers what is done with a link. Whoever reads a page they cannot write opens
 * a link with a press, as on any page. Only an address the page would take as a link is opened.
 */
import type { Editor } from '@tiptap/core';
import type { EditorView } from '@tiptap/pm/view';
import { useEditorState } from '@tiptap/react';
import { BubbleMenu, type BubbleMenuProps } from '@tiptap/react/menus';
import { floatingClasses, floatingPlacement } from '@/ui/Popover';
import { useTranslation } from 'react-i18next';

import { linkAllowed } from '@/features/documents/schema';
import { linkFieldOf } from '@/features/documents/TextBar';
import { actionName } from '@/ui/actions';
import { copyText } from '@/ui/clipboard';
import { IconButton } from '@/ui/IconButton';

/** Opens an address in a new tab of the browser, where it is one the page takes as a link. */
export function openLink(href: string): void {
  if (linkAllowed(href)) window.open(href, '_blank', 'noopener,noreferrer');
}

/** The address of the link at `pos`, if there is one. */
function linkAt(view: EditorView, pos: number): string | null {
  const type = view.state.schema.marks.link;
  if (type === undefined) return null;
  const mark = view.state.doc.resolve(pos).marks().find((one) => one.type === type);
  const href: unknown = mark?.attrs.href;
  return typeof href === 'string' && href !== '' ? href : null;
}

/** A press on a link: Ctrl+Click (⌘-click) opens it in a new tab, as does any press of somebody who cannot write the page. */
export function openLinkOnPress(view: EditorView, pos: number, event: MouseEvent): boolean {
  if (!(event.ctrlKey || event.metaKey) && view.editable) return false;
  const href = linkAt(view, pos);
  if (href === null) return false;
  openLink(href);
  return true;
}

/** The card shows while the caret, and no selection, is in a link of a page that has the focus. Held outside the component, so that the menu is not set up again on every render. */
const showInLink: NonNullable<BubbleMenuProps['shouldShow']> = ({ editor, view, state }) => view.hasFocus() && state.selection.empty && editor.isActive('link');
const CARD_PLACEMENT = { placement: 'bottom-start', offset: 6, ...floatingPlacement } as const;

export function LinkCard({ editor, editable }: { editor: Editor; editable: boolean }) {
  const { t } = useTranslation();
  const href = useEditorState({ editor, selector: ({ editor: current }) => (current.getAttributes('link').href as string | undefined) ?? '' });
  return (
    <BubbleMenu editor={editor} pluginKey="orchLinkCard" shouldShow={showInLink} options={CARD_PLACEMENT}>
      <div role="toolbar" aria-label={t('documents.link.card')} className={`flex items-center gap-1 overflow-y-auto rounded-lg border border-border bg-surface-raised py-1 pr-1 pl-3 shadow-md ${floatingClasses}`}>
        <span className="min-w-0 flex-1 truncate text-sm text-text-muted" title={href}>
          {href}
        </span>
        <IconButton action="openNewTab" label={actionName('openNewTab')} onClick={() => openLink(href)} />
        <IconButton action="copyLink" label={actionName('copyLink')} onClick={() => void copyText(href, t('common.linkCopied'))} />
        {editable && (
          <IconButton
            action="edit"
            label={actionName('edit')}
            onClick={() => {
              // The whole link is taken, and the field of the bar over it opens with its address.
              editor.chain().focus().extendMarkRange('link').run();
              linkFieldOf(editor).setState({ open: true });
            }}
          />
        )}
      </div>
    </BubbleMenu>
  );
}
