/** Shared ConfirmDialog component (access): the effect of an act stated before it, a dialog of several steps saying where one is and what its last button does, and the override hint when an admin of the place forces what a page does not give them.
 *
 * What is asked before something is archived, deleted, revoked, stopped or switched off: the thing by
 * its name, what will happen to it, and a button that repeats the verb and the name, so that nobody
 * confirms the wrong thing by habit. It asks once. The focus begins on the way out, never on the act.
 *
 * So far: the confirmation itself, and the line an admin reads when they force what a page does not
 * give them, with what they will be added to. The steps of a longer dialog are not built yet.
 */
import { AlertDialog as Primitive } from 'radix-ui';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { ContentFailure } from '@/ui/ContentBoundary';
import { ErrorBoundary } from '@/ui/ErrorBoundary';
import { overlayClasses, useOpener } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { AdminActNotice, type AdminActSpec } from '@/ui/AdminAct';
import { PromptScope, usePrompt } from '@/ui/PromptScope';

export interface ConfirmDialogProps {
  admin?: AdminActSpec;
  /** The reviewed reason or notification choice, before Cancel and the primary act. */
  footerStart?: ReactNode;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Names the thing: "Archive Fix login?" */
  title: ReactNode;
  /** What happens: "Stops 2 running threads; everything stays recoverable for 30 days." */
  children: ReactNode;
  /** The verb and the name once more: "Archive Fix login". */
  confirmLabel: string;
  /** Marks an act that destroys or cannot be taken back. */
  danger?: boolean;
  /** The act is under way: the dialog stays, and cannot be left or confirmed again. */
  pending?: boolean;
  /** A required review has not arrived, expired, or contains blockers. */
  disabled?: boolean;
  /** What the act was refused with, shown above the buttons. */
  error?: unknown;
  onConfirm: () => void;
  /** Present where the act is an admin forcing what the page does not give them. */
  override?: { hint: ReactNode; additions: readonly ReactNode[] };
}

export function ConfirmDialog({
  admin,
  footerStart,
  open,
  onOpenChange,
  title,
  children,
  confirmLabel,
  danger = false,
  pending = false,
  disabled = false,
  error,
  onConfirm,
  override,
}: ConfirmDialogProps) {
  const { t } = useTranslation();
  const layer = usePrompt(open);
  const opener = useOpener();
  return (
    <PromptScope layer={layer}><Primitive.Root open={open} onOpenChange={(next) => (pending ? undefined : onOpenChange(next))}>
      <Primitive.Portal>
        <Primitive.Overlay style={layer.style} className={overlayClasses} />
        <div style={layer.style} className="pointer-events-none fixed inset-0 z-(--layer-modal) grid place-items-center p-4">
          <Primitive.Content
            data-prompt-id={layer.id}
            data-prompt-current={!layer.covered}
            onOpenAutoFocus={opener.remember}
            onCloseAutoFocus={opener.giveBack}
            className="pointer-events-auto flex max-h-full w-full max-w-dialog-sm flex-col gap-3 overflow-y-auto rounded-xl border border-border bg-surface-raised p-6 shadow-lg data-[state=open]:animate-rise-in"
          >
            <Primitive.Title className="text-lg font-semibold text-text">{title}</Primitive.Title>
            <ErrorBoundary resetKey={open} fallback={(_error, retry) => <>
              <Primitive.Description asChild><div><ContentFailure retry={retry} /></div></Primitive.Description>
              <Primitive.Cancel asChild><Button disabled={pending}>{t('common.cancel')}</Button></Primitive.Cancel>
            </>}>
            <Primitive.Description asChild>
              <div className="text-base text-text-muted">{children}<AdminActNotice spec={admin} /></div>
            </Primitive.Description>
            {override !== undefined && (
              <InlineAlert tone="warning">
                <p>{override.hint}</p>
                {override.additions.length > 0 && (
                  <ul className="mt-1 list-disc pl-4">
                    {override.additions.map((addition, index) => (
                      // The list is given whole and never reordered while the dialog is open.
                      <li key={index}>{addition}</li>
                    ))}
                  </ul>
                )}
              </InlineAlert>
            )}
            {error !== undefined && error !== null && <ErrorAlert error={error} />}
            <div className="mt-2 flex flex-wrap items-center justify-end gap-2">
              {footerStart !== undefined && <div className="me-auto min-w-0">{footerStart}</div>}
              <div className="ms-auto flex min-w-0 max-w-full shrink-0 items-center gap-2">
                <Primitive.Cancel asChild>
                  <Button disabled={pending}>{t('common.cancel')}</Button>
                </Primitive.Cancel>
                <Button className="min-w-0" variant={danger ? 'danger' : 'primary'} pending={pending} disabled={disabled || pending} onClick={onConfirm}>
                  <span className="min-w-0 truncate">{confirmLabel}</span>
                </Button>
              </div>
            </div>
            </ErrorBoundary>
          </Primitive.Content>
        </div>
      </Primitive.Portal>
    </Primitive.Root></PromptScope>
  );
}
