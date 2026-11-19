/** Shared Drawer component (overlays); primitives are wrapped here.
 *
 * A sheet that comes in from an edge of the screen and lies over the page: the navigation on a small
 * screen, a bar that has no room beside the work, a view shown one at a time. Like a dialog it takes
 * the focus and gives it back, and Escape or a press beside it puts it away.
 */
import { X } from 'lucide-react';
import { Dialog as Primitive } from 'radix-ui';
import type { CSSProperties, ReactNode, PointerEventHandler, MouseEventHandler } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { overlayClasses, useOpener } from '@/ui/Dialog';
import { scrollAreaClasses } from '@/ui/ScrollArea';
import { PromptScope, usePrompt } from '@/ui/PromptScope';

export type DrawerSide = 'left' | 'right' | 'bottom';

export interface DrawerProps {
  open: boolean;
  /** Workbench views use a sheet as layout; they do not compete with a confirmation or form. */
  prompt?: boolean;
  /** A navigation view leaves the common title bar reachable while its sheet is open. */
  modal?: boolean;
  /** A view chosen for reading closes only when its reader presses Close or Escape. */
  persistent?: boolean;
  /** A new sheet may take the keyboard while this one is closing. */
  restoreFocus?: boolean;
  onOpenChange: (open: boolean) => void;
  /** What the sheet is, for its heading and for a screen reader. */
  title: ReactNode;
  /** Actions supplied by the shown view, beside its title. */
  actions?: ReactNode;
  /** Keeps the heading for screen readers only, where what is inside names itself. */
  hideTitle?: boolean;
  /** Workbench sheets delegate scrolling to their contained views. */
  scroll?: boolean;
  side?: DrawerSide;
  children: ReactNode;
  className?: string;
  /** Layout sheets can reserve their measured chrome without changing prompt stacking. */
  style?: CSSProperties;
  onPointerEnter?: PointerEventHandler<HTMLDivElement>;
  onPointerLeave?: PointerEventHandler<HTMLDivElement>;
  onClickCapture?: MouseEventHandler<HTMLDivElement>;
}

const SIDES: Record<DrawerSide, string> = {
  left: 'inset-y-0 left-0 h-full w-(--drawer-width) max-w-[calc(100%-var(--spacing-12))] border-r data-[state=open]:animate-sheet-left',
  right:
    'inset-y-0 right-0 h-full w-(--right-bar-width) max-w-[calc(100%-var(--spacing-12))] border-l data-[state=open]:animate-sheet-right',
  bottom:
    'inset-x-0 bottom-0 max-h-[calc(100%-var(--spacing-12))] w-full rounded-t-xl border-t data-[state=open]:animate-sheet-bottom',
};

export function Drawer({ open, prompt = true, modal = true, persistent = false, restoreFocus = true, onOpenChange, title, actions, hideTitle = false, scroll = true, side = 'left', children, className, style, onPointerEnter, onPointerLeave, onClickCapture }: DrawerProps) {
  const { t } = useTranslation();
  const layer = usePrompt(open && prompt);
  const opener = useOpener();
  return (
    <PromptScope layer={layer}><Primitive.Root open={open} modal={modal} onOpenChange={onOpenChange}>
      <Primitive.Portal>
        <Primitive.Overlay style={layer.style} className={overlayClasses} />
        <Primitive.Content
          onPointerEnter={onPointerEnter}
          onPointerLeave={onPointerLeave}
          onClickCapture={onClickCapture}
          data-prompt-id={layer.id}
          data-prompt-owner={layer.owner ?? undefined}
          style={{ ...style, ...layer.style }}
          data-prompt-current={prompt && !layer.covered}
          aria-describedby={undefined}
          onOpenAutoFocus={opener.remember}
          onCloseAutoFocus={(event) => { if (restoreFocus) opener.giveBack(event); else event.preventDefault(); }}
          onInteractOutside={(event) => { if (persistent) event.preventDefault(); }}
          className={cx(
            'fixed z-(--layer-modal) flex flex-col border-border bg-surface shadow-lg',
            SIDES[side],
            className,
          )}
        >
          <header
            className={cx(
              hideTitle ? 'contents' : 'flex h-(--tab-strip-height) shrink-0 items-center justify-between gap-2 border-b border-border pr-2 pl-4',
            )}
          >
            <Primitive.Title className={hideTitle ? 'sr-only' : 'truncate text-base font-semibold text-text'}>{title}</Primitive.Title>
            {!hideTitle && <ContentBoundary resetKey={open}>{actions}</ContentBoundary>}
            {!hideTitle && <Primitive.Close
              aria-label={t('common.close')}
              className="inline-flex size-(--control-sm) shrink-0 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-bg-muted hover:text-text"
            >
              <X aria-hidden />
            </Primitive.Close>}
          </header>
          <div className={cx('flex min-h-0 flex-1 flex-col', scroll ? scrollAreaClasses : 'relative overflow-hidden')}><ContentBoundary resetKey={open}>{children}</ContentBoundary></div>
        </Primitive.Content>
      </Primitive.Portal>
    </Primitive.Root></PromptScope>
  );
}
