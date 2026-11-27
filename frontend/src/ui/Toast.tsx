/** Shared Toast component (overlays); primitives are wrapped here.
 *
 * A short notice at the lower right for something that happened away from where its reader is
 * looking: a thing done in the background, or failed there. What fails beside its own control is
 * said beside that control, never here. A notice with an action stays until it is used or dismissed;
 * ordinary success leaves by itself, while failures stay until they are put away.
 */
import { PathLink } from '@/shell/TrailLink';
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

/** Something to do about a notice, on its one line of acts. */
export interface ToastAct {
  label: string;
  icon?: ReactNode;
  run: () => void;
  href?: string;
  /** Using it leaves the notice up (Show beside an Undo); every other act puts the notice away. */
  keep?: boolean;
}

export interface ToastOptions {
  tone: ToastTone;
  title: string;
  description?: string;
  /** A pending background act remains visible until its caller dismisses this notice on completion. */
  persistent?: boolean;
  /** What to do about it: one act, or a few on one line (Show · Undo). */
  action?: ToastAct | readonly ToastAct[];
}

/** The acts of a notice, in the order they stand. */
export function toastActs(action: ToastOptions['action']): readonly ToastAct[] {
  if (action === undefined) return [];
  return 'label' in action ? [action] : action;
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

const ACT_CLASSES = 'inline-flex items-center gap-1 rounded-sm font-medium text-accent-text hover:underline [&_svg]:size-4';

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
        const acts = toastActs(entry.action);
        return (
          <Primitive.Root
            key={entry.id}
            type={entry.tone === 'danger' ? 'foreground' : 'background'}
            duration={entry.tone === 'danger' || entry.persistent === true || acts.length > 0 ? Number.POSITIVE_INFINITY : TOAST_SHOWN_MS}
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
              {acts.length > 0 && (
                <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1">
                  {acts.map((act) => {
                    const label = act.href === undefined ? <>{act.icon}{act.label}</> : <PathLink href={act.href}>{act.icon}{act.label}</PathLink>;
                    return act.keep === true ? (
                      act.href === undefined
                        ? <button key={act.label} type="button" className={ACT_CLASSES} onClick={act.run}>{label}</button>
                        : <span key={act.label} className={ACT_CLASSES} onClickCapture={act.run}>{label}</span>
                    ) : (
                      <Primitive.Action key={act.label} asChild={act.href !== undefined} altText={act.label} className={ACT_CLASSES} onClick={act.run}>
                        {label}
                      </Primitive.Action>
                    );
                  })}
                </div>
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
