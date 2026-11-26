/** The one line a move to the trash leaves: what went, what running work it stopped, what was refused, Show and Undo.
 *
 * Nothing about what depends on the things is asked before the act; this line says it after, beside the act's
 * result, and Show opens where they now lie. Undo puts back exactly what this act moved.
 */
import type { TFunction } from 'i18next';

import { toast, type ToastAct } from '@/ui/Toast';

/** One thing that was not moved or not brought back, by its name and why. */
export interface Refusal {
  name: string;
  reason: string;
}

/** What Undo brought back, and what it could not. */
export interface UndoResult {
  restored: readonly string[];
  refused: readonly Refusal[];
}

export interface TrashTip {
  /** The names of what went to the trash, in the order they were chosen. */
  moved: readonly string[];
  /** What was not moved, each with its reason. */
  refused: readonly Refusal[];
  /** Running threads the act stopped: the one loss that is said. */
  stopped?: number;
  /** Opens the trash where it now lies. */
  show?: () => void;
  /** Puts back exactly what this act moved. */
  undo?: () => Promise<UndoResult>;
}

function refusedLine(t: TFunction, refused: readonly Refusal[]): string {
  return t('trash.refused', { names: refused.map((one) => t('trash.because', { name: one.name, reason: one.reason })).join('; ') });
}

/** The title and line of a trash notice. */
export function trashWords(t: TFunction, tip: Pick<TrashTip, 'moved' | 'refused' | 'stopped'>): { title: string; description?: string; failed: boolean } {
  const { moved, refused } = tip;
  const total = moved.length + refused.length;
  const [only] = moved;
  const title = moved.length === 0
    ? refused.length === 1 && refused[0] !== undefined ? t('trash.notMoved', { name: refused[0].name }) : t('trash.noneMoved')
    : refused.length > 0 ? t('trash.movedSome', { done: moved.length, count: total })
      : moved.length === 1 && only !== undefined ? t('trash.movedOne', { name: only }) : t('trash.moved', { count: moved.length });
  const parts = [
    (tip.stopped ?? 0) > 0 ? t('trash.stopped', { count: tip.stopped }) : null,
    refused.length === 0 ? null : moved.length === 0 && refused.length === 1 ? refused[0]?.reason ?? null : refusedLine(t, refused),
  ].filter((part): part is string => part !== null && part !== '');
  return { title, ...(parts.length === 0 ? {} : { description: parts.join(' · ') }), failed: moved.length === 0 };
}

/** Says what Undo did. */
export function sayUndone(t: TFunction, result: UndoResult): void {
  const [only] = result.restored;
  if (result.refused.length === 0) {
    toast({ tone: 'success', title: result.restored.length === 1 && only !== undefined ? t('trash.backOne', { name: only }) : t('trash.back', { count: result.restored.length }) });
    return;
  }
  toast({
    tone: 'danger',
    title: result.restored.length === 0 ? t('trash.notBack') : t('trash.backSome', { done: result.restored.length, count: result.restored.length + result.refused.length }),
    description: t('trash.notBackNames', { names: result.refused.map((one) => t('trash.because', { name: one.name, reason: one.reason })).join('; ') }),
  });
}

/** Leaves the one line of a move to the trash. */
export function sayTrashed(t: TFunction, tip: TrashTip): void {
  const words = trashWords(t, tip);
  const acts: ToastAct[] = [];
  if (tip.moved.length > 0 && tip.show !== undefined) acts.push({ label: t('trash.show'), keep: true, run: tip.show });
  const undo = tip.undo;
  if (tip.moved.length > 0 && undo !== undefined) {
    acts.push({
      label: t('trash.undo'),
      run: () => void undo().then((result) => sayUndone(t, result), (error: unknown) => sayUndone(t, { restored: [], refused: [{ name: tip.moved.join(', '), reason: error instanceof Error ? error.message : String(error) }] })),
    });
  }
  toast({
    tone: words.failed ? 'danger' : 'success',
    title: words.title,
    ...(words.description === undefined ? {} : { description: words.description }),
    ...(acts.length === 0 ? {} : { action: acts }),
  });
}
