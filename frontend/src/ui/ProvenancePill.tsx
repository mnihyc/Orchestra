/** Shared ProvenancePill component (status); primitives are wrapped here.
 *
 * Where the value in force comes from, in a few words beside it: the shipped default, a place above,
 * this very place, or a place above that locks it. The kind is told by an icon and by the words, never
 * by colour alone. What the words name (which org, which project) is the caller's to say.
 */
import { ArrowDownToLine, CircleDashed, Lock, PencilLine } from 'lucide-react';
import type { ReactNode } from 'react';

import { cx } from '@/ui/cx';

export type ProvenanceKind =
  /** Nothing was set anywhere: what the system ships, or supplies for the thing. */
  | 'default'
  /** Set in a place above. */
  | 'inherited'
  /** Set in this place. */
  | 'here'
  /** Set above, and not to be changed below. */
  | 'locked';

export interface ProvenancePillProps {
  kind: ProvenanceKind;
  /** The words: "Default", "Acme", "Set here". */
  children: ReactNode;
  className?: string;
}

const LOOKS: Record<ProvenanceKind, { Icon: typeof Lock; box: string }> = {
  default: { Icon: CircleDashed, box: 'border-border bg-bg-subtle text-text-muted' },
  inherited: { Icon: ArrowDownToLine, box: 'border-border bg-bg-muted text-text' },
  here: { Icon: PencilLine, box: 'border-accent/30 bg-accent-subtle text-text' },
  locked: { Icon: Lock, box: 'border-warning/40 bg-warning-subtle text-text' },
};

export function ProvenancePill({ kind, children, className }: ProvenancePillProps) {
  const { Icon, box } = LOOKS[kind];
  return (
    <span
      data-provenance={kind}
      className={cx('inline-flex max-w-full items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-medium', box, className)}
    >
      <Icon className="size-3 shrink-0" aria-hidden />
      <span className="truncate">{children}</span>
    </span>
  );
}
