/** The first overview request and its controls share these period and breakdown choices. */
import { useState } from 'react';
import type { UsageParams } from '@/api/usage';
import { useListChoice } from '@/lib/listState';
import { DEFAULT_PERIOD, PEOPLE_ASKED, boardSpan, type BoardPeriod, type OtherWay } from './boardParameters';

/** The overview and its native board use identical restored controls and bounds. */
export function useUsageBoardParams(nodeId: string): UsageParams {
  const [now] = useState(Date.now);
  // Read to ask for what the screen shows, whichever screen it is: no control of the board is here to put its choice into the address.
  const [period] = useListChoice<BoardPeriod>('usage-board', 'period', DEFAULT_PERIOD, 'address', false);
  const [other] = useListChoice<OtherWay>('usage-board', 'other', 'model', 'address', false);
  return { scope: `node:${nodeId}`, ...boardSpan(period, now), breakdowns: ['person', other], limit: PEOPLE_ASKED, filters: {} };
}

/** The compact overview and its native opening share the same weekly totals request. */
export function useWeeklyUsageParams(nodeId: string): UsageParams {
  const [now] = useState(Date.now);
  return { scope: `node:${nodeId}`, ...boardSpan('week', now), breakdowns: [], filters: {} };
}
