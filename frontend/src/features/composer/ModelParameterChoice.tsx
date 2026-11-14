/** The model's native choices use one control in the composer and in place defaults. */
import { ContextChoice } from '@/features/composer/ContextChoice';
import { AccessChoice, SpeedChoice } from '@/features/composer/SpeedAndAccess';
import type { Chooser, State } from '@/features/composer/modelState';
import type { Rate } from '@/generated/api-types';

export function ModelParameterChoice({ state, writable, chooser = null, rate = null, pending = false, reopen = false, performance, onChoose }: {
  state: State; writable: boolean; chooser?: Chooser | null; rate?: Rate | null; pending?: boolean; reopen?: boolean;
  performance?: (value: string | null) => string | null; onChoose: (value: string | number | null) => void;
}) {
  if (state.kind === 'context') return <ContextChoice state={state} writable={writable} chooser={chooser} onChoose={onChoose} />;
  if (state.kind === 'speed') return <SpeedChoice state={state} writable={writable} chooser={chooser} rate={rate} pending={pending} performance={performance} onChoose={onChoose} />;
  return <AccessChoice state={state} writable={writable} chooser={chooser} reopen={reopen} performance={performance} onChoose={onChoose} />;
}
