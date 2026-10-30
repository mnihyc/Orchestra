/** Shared Toast component (overlays); primitives are wrapped here.
 *
 * A short notice at the lower right for something that happened away from where its reader is
 * looking: a thing done in the background, or failed there. What fails beside its own control is
 * said beside that control, never here. A notice of success leaves by itself; a notice of failure
 * stays until it is put away, because a failure nobody saw is one nobody acts on.
 */
import { CircleAlert, CircleCheck, Info, X } from 'lucide-react';
import { Toast as Primitive } from 'radix-ui';
import { Activity, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { create } from 'zustand';

import { cx } from '@/ui/cx';
import { cardClasses } from '@/ui/Popover';
import { useNoticesPaused } from '@/ui/PromptScope';

/** How long a notice that is no failure stays. */
export const TOAST_SHOWN_MS = 4_000;

export type ToastTone = 'success' | 'info' | 'danger';

export interface ToastOptions {
  tone: ToastTone;
  title: string;
  description?: string;
  /** A pending background act remains visible until its caller dismisses this notice on completion. */
  persistent?: boolean;
  /** One thing to do about it. */
  action?: { label: string; icon?: ReactNode; run: () => void };
}

interface Shown extends ToastOptions {
  id: number;
}

interface ToastStore {
  shown: Shown[];
  next: number;
}

const useToasts = create<ToastStore>(() => ({ shown: [], next: 1 }));

/** Shows a notice. What it returns puts it away. */
export function toast(options: ToastOptions): () => void {
  const id = useToasts.getState().next;
  useToasts.setState((state) => ({ shown: [...state.shown, { ...options, id }], next: id + 1 }));
  return () => dismiss(id);
}

function dismiss(id: number): void {
  useToasts.setState((state) => ({ shown: state.shown.filter((entry) => entry.id !== id) }));
}

const TONES: Record<ToastTone, { icon: string; Icon: typeof Info }> = {
  success: { icon: 'text-success', Icon: CircleCheck },
  info: { icon: 'text-info', Icon: Info },
  danger: { icon: 'text-danger', Icon: CircleAlert },
};

/** Stands once in the application and draws the notices that are showing. */
export function Toaster({ children }: { children?: ReactNode }) {
  const { t } = useTranslation();
  const shown = useToasts((state) => state.shown);
  const prompting = useNoticesPaused();
  return (
    <Primitive.Provider label={t('ui.toast.label')} duration={TOAST_SHOWN_MS} swipeDirection="right">
      {children}
      <Activity mode={prompting ? 'hidden' : 'visible'}>
      {shown.map((entry) => {
        const { icon, Icon } = TONES[entry.tone];
        return (
          <Primitive.Root
            key={entry.id}
            type={entry.tone === 'danger' ? 'foreground' : 'background'}
            duration={entry.tone === 'danger' || entry.persistent === true ? Number.POSITIVE_INFINITY : TOAST_SHOWN_MS}
            onOpenChange={(open) => {
              if (!open) dismiss(entry.id);
            }}
            className={cx('flex animate-rise-in items-start gap-2 p-3 text-sm text-text', cardClasses)}
          >
            <Icon className={cx('mt-0.5', icon)} aria-hidden />
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <Primitive.Title className="font-medium wrap-anywhere">{entry.title}</Primitive.Title>
              {entry.description !== undefined && (
                <Primitive.Description className="wrap-anywhere text-text-muted">
                  {entry.description}
                </Primitive.Description>
              )}
              {entry.action !== undefined && (
                <Primitive.Action
                  altText={entry.action.label}
                  className="mt-1 inline-flex self-start items-center gap-1 rounded-sm font-medium text-accent-text hover:underline [&_svg]:size-4"
                  onClick={entry.action.run}
                >
                  {entry.action.icon}
                  {entry.action.label}
                </Primitive.Action>
              )}
            </div>
            <Primitive.Close
              aria-label={t('common.dismiss')}
              className="-mt-1 -mr-1 inline-flex size-(--control-sm) shrink-0 items-center justify-center rounded-sm text-text-muted transition-colors hover:bg-bg-muted hover:text-text"
            >
              <X aria-hidden />
            </Primitive.Close>
          </Primitive.Root>
        );
      })}
      <Primitive.Viewport className="fixed right-0 bottom-0 z-(--layer-toast) flex w-full max-w-(--container-form) flex-col gap-2 p-4 outline-none" />
      </Activity>
    </Primitive.Provider>
  );
}
