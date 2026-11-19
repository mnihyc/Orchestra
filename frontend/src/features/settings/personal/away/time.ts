/** Convert an explicitly entered wall time in the shown zone without guessing missing or repeated hours. */
const DAY_MS = 24 * 60 * 60_000;

/** A stored end in the past (or an invalid old value) is no longer a pause. */
export function activePause(until: string | null, now: number): string | null {
  return until !== null && Date.parse(until) > now ? until : null;
}

function wallValue(instant: number, zone: string): number {
  const parts = new Intl.DateTimeFormat('en-GB', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(instant);
  const part = (type: string) => Number(parts.find((entry) => entry.type === type)?.value);
  return Date.UTC(part('year'), part('month') - 1, part('day'), part('hour'), part('minute'));
}

export type TimeChoice = { instant: string } | { error: 'missing' | 'invalid' | 'ambiguous' | 'future' };

/** Adjacent days provide both sides of an offset transition; every candidate must round-trip to the entered calendar minute. */
export function futureInZone(date: string, time: string, zone: string, now: number): TimeChoice {
  if (date === '' || time === '') return { error: 'missing' };
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date) || !/^\d{2}:\d{2}$/.test(time)) return { error: 'invalid' };
  const entered = `${date}T${time}:00.000Z`;
  const wall = Date.parse(entered);
  if (!Number.isFinite(wall) || new Date(wall).toISOString() !== entered) return { error: 'invalid' };
  try {
    const offsets = new Set([-DAY_MS, 0, DAY_MS].map((delta) => wallValue(wall + delta, zone) - (wall + delta)));
    const candidates = [...offsets].map((offset) => wall - offset).filter((instant) => wallValue(instant, zone) === wall);
    if (candidates.length === 0) return { error: 'invalid' };
    if (candidates.length > 1) return { error: 'ambiguous' };
    const instant = candidates[0];
    if (instant === undefined || instant <= now) return { error: 'future' };
    return { instant: new Date(instant).toISOString() };
  } catch { return { error: 'invalid' }; }
}

export function writePause(at: string, zone: string | undefined, locale: string): string {
  return new Intl.DateTimeFormat(locale, { timeZone: zone, dateStyle: 'medium', timeStyle: 'short' }).format(new Date(at));
}
