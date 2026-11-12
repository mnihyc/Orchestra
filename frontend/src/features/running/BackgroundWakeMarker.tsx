/** A continuation's first line is a recorded event, never text supplied by the model. */
import { Pause, Play } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { MarkerBackgroundWake } from '@/generated/api-types';
import { heldInputWords } from '@/features/composer/heldInputWords';
import { useThreadPart } from '@/realtime/stores/thread';
import { TimeOfDay } from '@/ui/TimeOfDay';
import { wakeCauses } from './backgroundWake';

export function BackgroundWakeMarker({ threadId, data, at }: { threadId: string; data: MarkerBackgroundWake; at: string }) {
  const { t } = useTranslation();
  const input = useThreadPart(threadId, (view) => view.inputs.get(data.input_id));
  const thread = useThreadPart(threadId, (view) => view.thread);
  const causes = wakeCauses(data.causes, t) || t('backgroundWake.work');
  const held = !data.admitted && input !== undefined ? heldInputWords(input, thread, t) : null;
  const Icon = data.admitted ? Play : Pause;
  const words = data.admitted ? t('backgroundWake.wokenBy', { causes }) : held === null ? t('backgroundWake.waitingBy', { causes }) : `${held} · ${causes}`;
  return <div className="flex min-w-0 flex-wrap items-center justify-center gap-1 py-1 text-center text-sm text-text-muted" data-background-wake={data.admitted ? 'admitted' : 'held'}>
    <Icon className="size-4 shrink-0" aria-hidden /><span className="min-w-0 wrap-anywhere whitespace-normal">{words}</span><TimeOfDay at={at} className="text-xs" />
  </div>;
}
