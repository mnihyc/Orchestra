/** The one sentence a change opens on, with its choices as menus: what it does, on what, where, and how much of the history.
 *
 * "Continue this thread [on Team Pro · gpt-… · high] [in a new environment on host-3]": most people need nothing
 * more. Each choice in the sentence is a button that opens its editor below the sentence, one at a time, and
 * says by its look when what it holds is not what the thread has now. The sentence is the heading of the plan:
 * everything under it is what it comes to.
 */
import { ChevronDown } from 'lucide-react';
import { type ReactNode, type RefObject, useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { buttonClasses } from '@/ui/Button';
import { cx } from '@/ui/cx';

export interface ChipProps {
  /** What it chooses, for a screen reader: "Provider and model". */
  label: string;
  /** What it holds now, as the sentence reads it. */
  children: ReactNode;
  open: boolean;
  /** What it holds differs from what the thread has: the chip is drawn in the accent. */
  changed?: boolean;
  /** Cannot be opened, and says why in its title. */
  blocked?: string;
  controls: string;
  onToggle: () => void;
}

/** One choice of the sentence. */
export function Chip({ label, children, open, changed = false, blocked, controls, onToggle }: ChipProps) {
  const { t } = useTranslation();
  const valueId = useId();
  const reasonId = `${valueId}-reason`;
  const value = typeof children === 'string' || typeof children === 'number' ? String(children) : null;
  const valueTitle = value === null ? undefined : t('changeControl.value', { value });
  const title = blocked === undefined ? valueTitle : valueTitle === undefined ? blocked : `${valueTitle}\n${blocked}`;
  return (
    <>
      <button
        type="button"
        aria-label={label}
        aria-expanded={open}
        aria-controls={controls}
        aria-describedby={blocked === undefined ? valueId : `${valueId} ${reasonId}`}
        title={title}
        aria-disabled={blocked === undefined ? undefined : true}
        onClick={blocked === undefined ? onToggle : undefined}
        data-change-choice
        data-changed={changed ? 'true' : undefined}
        className={cx(
          buttonClasses({ variant: 'secondary', size: 'md', disabled: blocked !== undefined }),
          'min-w-0 w-fit max-w-full self-start justify-start align-middle text-left',
          blocked === undefined && changed && 'data-[changed=true]:border-accent data-[changed=true]:bg-accent-subtle data-[changed=true]:text-accent-text data-[changed=true]:hover:bg-accent-subtle',
          open && 'ring-2 ring-focus',
          blocked !== undefined && 'cursor-not-allowed text-text-muted hover:bg-surface',
        )}
      >
        <span id={valueId} className="min-w-0 truncate">{children}</span>
        <ChevronDown aria-hidden className={cx('shrink-0 transition-transform', open && 'rotate-180')} />
      </button>
      {blocked !== undefined && <span id={reasonId} className="sr-only">{blocked}</span>}
    </>
  );
}

/** What takes the focus of an editor that opens: the choice that is made in a group of radios, else the first control. */
const CHECKED = '[role="radio"][aria-checked="true"]';
const CONTROLS = '[role="radio"], input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [role="combobox"]:not([disabled]), button:not([disabled])';

/**
 * Moves the focus into what has just opened, so that a person at the keyboard who opened it from a choice in the sentence is in it
 * (and not left to cross the other choices first), and a person at a screen reader hears where they are. Give the ref to its root.
 */
export function useFocusFirst<T extends HTMLElement>(enabled = true): RefObject<T | null> {
  const ref = useRef<T | null>(null);
  useEffect(() => {
    const root = ref.current;
    if (!enabled || root === null) return;
    (root.querySelector<HTMLElement>(CHECKED) ?? root.querySelector<HTMLElement>(CONTROLS))?.focus();
  }, [enabled]);
  return ref;
}

/** Ordinary text and controls wrap together without inheriting a heading's height. */
export function Sentence({ label, children }: { label: string; children: ReactNode }) {
  return (
    <p role="group" aria-label={label} className="flex min-w-0 flex-wrap items-center gap-x-1 gap-y-2 text-base text-text">
      {children}
    </p>
  );
}

/** What a choice opens: the editor of one part of the sentence, drawn under the sentence. */
export function Editor({ id, title, about, hint, children }: { id: string; title: string; about: string; hint?: ReactNode; children: ReactNode }) {
  const ref = useFocusFirst<HTMLElement>();
  return (
    <section ref={ref} id={id} aria-label={title} className="flex flex-col gap-3 rounded-lg border border-border bg-bg-subtle p-4">
      <div className="flex flex-col">
        <div className="flex items-center gap-1">
          <h3 className="text-base font-semibold text-text">{title}</h3>
          {hint}
        </div>
        <p className="text-sm text-text-muted">{about}</p>
      </div>
      {children}
    </section>
  );
}
