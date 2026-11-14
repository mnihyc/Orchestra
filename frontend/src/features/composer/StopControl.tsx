/** Explicit and keyboard interruption, for whoever may drive the thread.
 *
 * Stopping the turn that runs, or the start of the next one while there is no turn yet, for whoever may talk to the agent:
 * a button, and the key. With an empty box and a turn running, the first press of `Esc` says "Press Esc again to stop", and a
 * second within two seconds stops it. The server answers at once and says that the thread is stopping; the button says
 * "Stopping…", with the time that has gone by once it lasts more than a second, until the program has stopped.
 */
import { useCompactControls } from '@/features/workbench/Narrow';
import { useMutation } from '@tanstack/react-query';
import { Square } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { interruptTurn } from '@/api/threads';
import type { Turn } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { runningTurn, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import type { MenuEntry } from '@/ui/Menu';
import { SplitButton } from '@/ui/SplitButton';

/** The second press of Esc has to come within this long after the first. */
export const ESC_AGAIN_WITHIN_MS = 2_000;

/** Since when a stop has been asked for and the program has not stopped: the turn's own mark where it has one, else what the thread says
 * (a Stop that waits for the thread's row, or the Stop of a start that has no turn); null while nobody is stopping it. */
export function stoppingSince(turn: Turn | null, thread: string | null): string | null {
  return turn !== null && turn.interrupted_by !== null ? turn.interrupted_at ?? thread : thread;
}

/** The stop of a turn, or of a start that has no turn yet (`starting`), and the Esc that does it. */
export function useStop(threadId: string, turnId: string | null, live: boolean, starting = false) {
  const read = useThreadReadRef(threadId);
  const [armed, setArmed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const storedSince = useThreadPart(threadId, (view) => stoppingSince(runningTurn(view), view.thread?.stopping_since ?? null));
  const stop = useMutation({
    mutationFn: () => interruptTurn(read, turnId),
    onSuccess: (answer) => {
      if (answer.turn !== undefined && answer.turn !== null) threadWriter.apply(read, { turns: [answer.turn] });
    },
  });
  const may = turnId !== null || starting;
  const answeredTurn = stop.data?.turn;
  const answerNamesTarget = answeredTurn?.id === turnId || starting && (answeredTurn === null || answeredTurn === undefined);
  const since = storedSince ?? (may && answerNamesTarget && stop.data?.state === 'stopping' ? stop.data.requested_at : null);
  const reset = stop.reset;
  useEffect(() => { if (!may) reset(); }, [may, reset]);
  useEffect(() => () => clearTimeout(timer.current), []);
  /** Called for an Esc in an empty box: the first press arms, the second stops. */
  const escape = useCallback(() => {
    if (!may || !live || stop.isPending) return;
    if (armed) {
      clearTimeout(timer.current);
      setArmed(false);
      stop.mutate();
      return;
    }
    setArmed(true);
    timer.current = setTimeout(() => setArmed(false), ESC_AGAIN_WITHIN_MS);
  }, [armed, may, live, stop]);
  return { stop, armed, escape, since, may };
}

/** What the Stop button says: its own word, and while the stop is under way "Stopping…" with the time once it lasts more than a second. */
function useStopLabel(stop: ReturnType<typeof useStop>): { label: string; stopping: boolean } {
  const { t } = useTranslation();
  const format = useFormatContext();
  const stopping = stop.stop.isPending || stop.since !== null;
  const now = useRunningClock(stopping);
  if (!stopping) return { label: t('composer.stop'), stopping };
  const since = stop.since === null ? Number.NaN : Date.parse(stop.since);
  const lasted = Number.isFinite(since) ? now - since : 0;
  return { label: lasted >= 1_000 ? t('composer.stoppingTime', { time: formatSpan(lasted, format) }) : t('composer.stopping'), stopping };
}

export function StopControl({ stop, entries, disabled = false, menuDisabled, reason, inlineFeedback = true }: {
  stop: ReturnType<typeof useStop>;
  entries?: readonly MenuEntry[];
  disabled?: boolean;
  /** Delivery choices can be ready before the current turn can be stopped. */
  menuDisabled?: boolean;
  reason?: string | undefined;
  inlineFeedback?: boolean;
}) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  const { label, stopping } = useStopLabel(stop);
  return (
    <>
      {entries === undefined ? <Button size="sm" data-tutorial-target="thread-stop" variant="secondary" icon={<Square aria-hidden />} pending={stopping} disabled={disabled} onClick={() => stop.stop.mutate()}>
        {label}
      </Button> : <SplitButton size="sm" variant="secondary" className={compact ? '[&_button]:gap-1 [&_button]:px-1 [&_button]:text-xs [&_button[data-action=stop]>svg:not(.animate-spin)]:hidden' : undefined} icon={<Square aria-hidden />} pending={stopping} disabled={disabled}
        menuDisabled={(menuDisabled ?? disabled) || stop.stop.isPending} tutorialTarget="thread-stop" menuLabel={t('composer.deliveryMenu')} entries={entries} onClick={() => stop.stop.mutate()} {...(reason === undefined ? {} : { reason })}>
        {label}
      </SplitButton>}
      {inlineFeedback && <StopFeedback stop={stop} />}
    </>
  );
}

/** Stop confirmation and failures stand below the row, where they cannot push Send off a phone. */
export function StopFeedback({ stop }: { stop: ReturnType<typeof useStop> }) {
  const { t } = useTranslation();
  return <>
    {stop.armed && (
        <span role="status" className="text-sm text-text-muted">
          {t('composer.escAgain')}
        </span>
      )}
      {stop.stop.error !== null && <ErrorAlert error={stop.stop.error} className="w-full" />}
  </>;
}
