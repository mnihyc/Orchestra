/** Pure overview bounds shared by addressed reads and their mounted controls. */
import type { UsageDimension } from '@/generated/api-types';
import { spanOf, today } from '@/features/usage/search';
import { DAY_MS } from '@/lib/time';

/** The periods the board offers. */
export const BOARD_PERIODS = ['day', 'week', 'last_week', 'month', 'year', 'all'] as const;
export type BoardPeriod = (typeof BOARD_PERIODS)[number];

/** What a board opens on: this week. */
export const DEFAULT_PERIOD: BoardPeriod = 'week';

/** The other ways the board divides the same total. */
export const OTHER_WAYS = ['model','offering','purpose', 'provider', 'agent', 'environment'] as const satisfies readonly UsageDimension[];
export type OtherWay = (typeof OTHER_WAYS)[number];


/** The existing overview request defaults to the ten people shown before its full usage page. */
export const PEOPLE_ASKED = 10;

/** The days a period covers, as of today, in UTC. */
export function boardSpan(period: BoardPeriod, now: number): { from: string; to: string } {
  const to = today(now);
  const midnight = Date.parse(`${to}T00:00:00Z`);
  const date = new Date(midnight);
  switch (period) {
    case 'day':
      return { from: to, to };
    case 'week':
      return { from: new Date(midnight - ((date.getUTCDay() + 6) % 7) * DAY_MS).toISOString().slice(0, 10), to };
    case 'last_week': {
      const span=spanOf({range:'last_week'},now);
      return {from:span.from,to:span.to};
    }
    case 'month':
      return { from: `${to.slice(0, 8)}01`, to };
    case 'year':
      return { from: `${to.slice(0, 4)}-01-01`, to };
    case 'all':
      return { from: '2020-01-01', to };
  }
}
