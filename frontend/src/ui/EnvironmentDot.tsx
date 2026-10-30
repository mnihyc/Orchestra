/** Shared EnvironmentDot component (status): how an environment stands, as a dot in the colour of its state and its name in words.
 *
 * One dot and one set of words for the nine states an environment can be in, so that a topic, a thread, the
 * dialog that starts one and the pages of resources say the same thing in the same colour. The colour never
 * stands alone: the words are written beside the dot where there is room, and otherwise are the name a
 * screen reader says and the pointer reads, with the environment's own name before them where it is given.
 */
import { useTranslation } from 'react-i18next';

import type { EnvironmentStatus } from '@/generated/api-types';
import { type DotTone, StatusDot } from '@/ui/StatusDot';

/**
 * The colour each state is drawn in: running is well, working through a change is under way, failing is not,
 * and one that is not there to be used (never started, stopped, offline, switched off) is quiet.
 */
export const ENVIRONMENT_TONES: Record<EnvironmentStatus, DotTone> = {
  none: 'idle',
  provisioning: 'info',
  running: 'success',
  stopped: 'idle',
  recreating: 'info',
  moving: 'info',
  failed: 'danger',
  offline: 'off',
  disabled: 'off',
  removed: 'off',
  lost: 'warning',
};

export interface EnvironmentDotProps {
  status: EnvironmentStatus;
  /** The environment's name, put before the state in what the dot says where the words are not written: "Workspace: Running". */
  name?: string;
  /** Writes the state beside the dot instead of keeping it for screen readers and the pointer. */
  showLabel?: boolean;
  className?: string;
}

export function EnvironmentDot({ status, name, showLabel = false, className }: EnvironmentDotProps) {
  const { t } = useTranslation();
  const state = t(`environments.status.${status}`);
  return <StatusDot tone={ENVIRONMENT_TONES[status]} label={!showLabel && name !== undefined ? `${name}: ${state}` : state} showLabel={showLabel} className={className} />;
}
