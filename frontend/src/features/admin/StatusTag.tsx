/** A few words that mark a row: that a person is suspended, that an org is personal, that a host is full. The words are always written; the tone only helps.
 *
 * The shared badge is a pill for counts and one or two short words; the phrases of Management ("Behind in
 * 2 things", "Ships with Orch") need to keep to one line whatever room they are in, which this does.
 */
import type { ReactNode } from 'react';

import { StatusChip } from '@/ui/StateChip';

export type TagTone = 'neutral' | 'accent' | 'warning' | 'danger';

export function StatusTag({ tone = 'neutral', children, className }: { tone?: TagTone; children: ReactNode; className?: string }) {
  return <StatusChip tone={tone} className={className}>{children}</StatusChip>;
}
