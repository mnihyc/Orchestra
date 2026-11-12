/** One split action: Send when idle, Stop while a turn runs, with explicit queue, steer and interruption choices. */
import { useCompactControls } from '@/features/workbench/Narrow';
import { CalendarClock, SendHorizontal } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { deliveryEntries, inputUnavailable, type DeliveryState } from '@/features/composer/DeliveryMenu';
import { StopControl, type useStop } from '@/features/composer/StopControl';
import type { Delivery } from '@/generated/api-types';
import type { MenuEntry } from '@/ui/Menu';
import { SplitButton } from '@/ui/SplitButton';
import { Tooltip } from '@/ui/Tooltip';

export interface DeliveryControlProps {
  state: DeliveryState;
  stop: ReturnType<typeof useStop>;
  turnId: string | null;
  mayDrive: boolean;
  empty: boolean;
  tooLong: boolean;
  attachmentsWaiting: boolean;
  onSend: (delivery: Delivery) => void;
  onSchedule?: (() => void) | undefined;
  scheduleUnavailable?: string | false;
  onAskOnSide?: (() => void) | undefined;
  /** A native side-mode or draft refusal shown before an aside can be chosen. */
  sideUnavailable?: string | false;
}

export function DeliveryControl({ state, stop, turnId, mayDrive, empty, tooLong, attachmentsWaiting, onSend, onSchedule, scheduleUnavailable, onAskOnSide, sideUnavailable = false }: DeliveryControlProps) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  const unavailable = inputUnavailable(state, { mayDrive, empty, tooLong, attachmentsWaiting }, t);
  const entries = deliveryEntries(state, onSend, t, onAskOnSide)
    .map<MenuEntry>((entry) => {
      if (!('onSelect' in entry)) return entry;
      const reason = entry.id === 'side' && sideUnavailable !== false ? sideUnavailable : entry.disabled;
      return { ...entry, disabled: typeof reason === 'string' && reason !== '' ? reason : unavailable };
    });
  if (onSchedule !== undefined) entries.push({ type: 'separator', id: 'before-schedule' }, { id: 'send-later', label: t('sendLater.title'), icon: <CalendarClock aria-hidden />, disabled: scheduleUnavailable ?? unavailable, onSelect: onSchedule });
  if (state.active && mayDrive) {
    const disabled = !state.live || turnId === null;
    const reason = !state.live ? t('composer.reason.offline') : turnId === null ? t('composer.row.waitingForTurn') : undefined;
    return <Tooltip content={reason}><span className="inline-flex" tabIndex={disabled ? 0 : undefined}>
      <StopControl stop={stop} entries={entries} inlineFeedback={false} disabled={disabled} menuDisabled={!state.live} />
    </span></Tooltip>;
  }
  const disabled = unavailable !== false;
  return <Tooltip content={unavailable === false ? undefined : unavailable}><span className="inline-flex" tabIndex={disabled ? 0 : undefined}>
    <SplitButton size="sm" variant="primary" className={compact ? '[&_button]:gap-1 [&_button]:px-1 [&_button]:text-xs [&_button[data-action=send]>svg]:hidden' : undefined} icon={<SendHorizontal aria-hidden />} menuLabel={t('composer.deliveryMenu')}
      disabled={disabled} menuDisabled={disabled && (onSchedule === undefined || (scheduleUnavailable ?? unavailable) !== false)}
      onClick={() => onSend('queue')} entries={entries}>{t('composer.send')}</SplitButton>
  </span></Tooltip>;
}
