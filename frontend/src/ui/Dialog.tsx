/** Shared Dialog component (overlays); primitives are wrapped here.
 *
 * Something that is asked or shown above the page and has to be answered or closed before the page
 * is used again. The focus goes into it, stays in it, and returns to where it came from; Escape and
 * a press beside it close it, unless it says that it is not to be left that way.
 */
import { X } from 'lucide-react';
import { Dialog as Primitive } from 'radix-ui';
import { type ReactNode, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { useScrolls } from '@/ui/useScrolls';
import { currentPrompt, PromptScope, restorePromptFocus, usePrompt } from '@/ui/PromptScope';

export type DialogSize = 'sm' | 'md' | 'lg' | 'xl';

export interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  /** One line under the title that says what this is for. */
  description?: ReactNode;
  size?: DialogSize;
  children?: ReactNode;
  /** The buttons, at the foot; the one that does the thing comes last. */
  footer?: ReactNode;
  /** False while an act is under way that must not be walked away from by accident. */
  dismissible?: boolean;
}

const SIZES: Record<DialogSize, string> = {
  sm: 'max-w-dialog-sm',
  md: 'max-w-dialog-md',
  lg: 'max-w-dialog-lg',
  xl: 'max-w-dialog-xl',
};

/**
 * Gives the keyboard back to whatever had it when something opened above the page. What opens is
 * told to open by a button, a menu entry or a key, and none of them is known to it: so it remembers,
 * as it opens, where the keyboard was, and returns it there when it closes.
 */
export function useOpener(): { remember: () => void; giveBack: (event: Event) => void } {
  const opener = useRef<Element | null>(null);
  return {
    remember: () => {
      if (opener.current?.isConnected !== true) opener.current = document.activeElement;
    },
    giveBack: (event) => {
      const element = opener.current;
      // A retired menu opener returns to the current form; paused work keeps its original opener.
      if (!(element instanceof HTMLElement) || !restorePromptFocus(element)) {
        event.preventDefault();
        const current = currentPrompt();
        if (current !== null && !current.contains(document.activeElement)) {
          (current.querySelector<HTMLElement>('[data-autofocus], input:not([type="hidden"]), textarea, select, button') ?? current).focus();
        }
        return;
      }
      opener.current = null;
      event.preventDefault();
      element.focus();
    },
  };
}

export const overlayClasses =
  'fixed inset-0 z-(--layer-overlay) bg-overlay data-[state=open]:animate-overlay-in data-[state=closed]:animate-overlay-out';

export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  size = 'md',
  children,
  footer,
  dismissible = true,
}: DialogProps) {
  const { t } = useTranslation();
  const layer = usePrompt(open);
  const opener = useOpener();
  const { attach, scrolls, scrollProps } = useScrolls();
  const stay = (event: Event) => {
    if (!dismissible) event.preventDefault();
  };
  return (
    <PromptScope layer={layer}><Primitive.Root open={open} onOpenChange={onOpenChange}>
      <Primitive.Portal>
        <Primitive.Overlay style={layer.style} className={overlayClasses} />
        <div style={layer.style} className="pointer-events-none fixed inset-0 z-(--layer-modal) grid grid-cols-[minmax(0,1fr)] grid-rows-[minmax(0,1fr)] place-items-center p-4">
          <Primitive.Content
            data-prompt-id={layer.id}
            data-prompt-current={!layer.covered}
            // A dialog without a describing line says so, instead of leaving a dangling reference.
            {...(description === undefined ? { 'aria-describedby': undefined } : {})}
            onCloseAutoFocus={opener.giveBack}
            onOpenAutoFocus={(event) => {
              opener.remember();
              // The keyboard begins where the dialog asks for something: at the field marked for it,
              // else at its first field. A dialog that asks for nothing begins at its first button.
              const content = event.currentTarget as HTMLElement;
              const first =
                content.querySelector<HTMLElement>('[data-autofocus]') ??
                content.querySelector<HTMLElement>('input:not([type="hidden"]), textarea, select');
              if (first === null) return;
              event.preventDefault();
              first.focus();
            }}
            onEscapeKeyDown={stay}
            onPointerDownOutside={stay}
            onInteractOutside={stay}
            className={cx(
              'pointer-events-auto flex max-h-(--dialog-max-height) min-w-0 w-full flex-col rounded-xl border border-border bg-surface-raised shadow-lg data-[state=open]:animate-rise-in',
              SIZES[size],
            )}
          >
            <header className="flex items-start gap-4 px-6 pt-5 pb-3">
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <Primitive.Title className="text-lg leading-(--control-sm) font-semibold text-text">{title}</Primitive.Title>
                {description !== undefined && (
                  <Primitive.Description className="text-sm text-text-muted">{description}</Primitive.Description>
                )}
              </div>
              {dismissible && (
                <Primitive.Close
                  aria-label={t('common.close')}
                  className="-mr-2 inline-flex size-(--control-sm) shrink-0 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-bg-muted hover:text-text"
                >
                  <X aria-hidden />
                </Primitive.Close>
              )}
            </header>
            <ContentBoundary resetKey={open} className="px-6 pb-5">
            {children !== undefined && (
              // A body that is longer than the window allows scrolls, and then takes the keyboard and is named for the dialog, so that what lies below can be read without a pointer.
              <div
                ref={attach}
                {...scrollProps}
                {...(scrolls ? { role: 'region', ...(typeof title === 'string' ? { 'aria-label': title } : {}) } : {})}
                className="focus-inset min-h-0 flex-1 overflow-y-auto px-6 py-2"
              >
                {children}
              </div>
            )}
            {footer !== undefined ? (
              <footer className="flex flex-wrap items-center justify-end gap-2 px-6 pt-3 pb-5">{footer}</footer>
            ) : (
              <div className="pb-4" />
            )}
            </ContentBoundary>
          </Primitive.Content>
        </div>
      </Primitive.Portal>
    </Primitive.Root></PromptScope>
  );
}
