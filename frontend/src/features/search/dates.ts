/** Dates as the search takes them: a day chosen in a field, turned into the moment it begins or ends in the reader's own time zone.
 *
 * The API asks for moments, and a person thinks in days. "From 1 October" means from the first moment of
 * that day where the person is, and "to 1 October" up to its last. The offset of a zone changes during a
 * year, so it is read for the day itself.
 */

/** The wall clock of a moment in a zone, as numbers. */
function parts(at: Date, timeZone: string): { year: number; month: number; day: number; hour: number; minute: number; second: number } {
  const found = new Intl.DateTimeFormat('en-US', {
    timeZone,
    hourCycle: 'h23',
    year: 'numeric',
    month: 'numeric',
    day: 'numeric',
    hour: 'numeric',
    minute: 'numeric',
    second: 'numeric',
  }).formatToParts(at);
  const get = (type: string): number => Number(found.find((part) => part.type === type)?.value ?? 0);
  return { year: get('year'), month: get('month'), day: get('day'), hour: get('hour'), minute: get('minute'), second: get('second') };
}

/** How far the wall clock of a zone is ahead of UTC at a moment, in milliseconds. */
function offsetAt(at: Date, timeZone: string): number {
  const wall = parts(at, timeZone);
  return Date.UTC(wall.year, wall.month - 1, wall.day, wall.hour, wall.minute, wall.second) - Math.floor(at.getTime() / 1_000) * 1_000;
}

/** The moment a day (`2026-10-01`) begins in a zone, or ends in it (its last millisecond). */
export function momentOfDay(day: string, timeZone: string, edge: 'start' | 'end'): string | undefined {
  const found = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (found === null) return undefined;
  const [year, month, date] = [Number(found[1]), Number(found[2]), Number(found[3])];
  const wall = edge === 'start' ? Date.UTC(year, month - 1, date, 0, 0, 0, 0) : Date.UTC(year, month - 1, date, 23, 59, 59, 999);
  let guess = new Date(wall);
  try {
    // The offset is read at the moment found by the first guess, which corrects it where a zone changes its clocks that day.
    for (let round = 0; round < 2; round += 1) guess = new Date(wall - offsetAt(guess, timeZone));
  } catch {
    // A zone the browser does not know: the day is taken in UTC.
    guess = new Date(wall);
  }
  return guess.toISOString();
}

/** The day a moment falls on in a zone, as a field holds it (`2026-10-01`). */
export function dayOfMoment(moment: string, timeZone: string): string {
  const at = new Date(moment);
  if (Number.isNaN(at.getTime())) return '';
  try {
    const wall = parts(at, timeZone);
    return `${String(wall.year).padStart(4, '0')}-${String(wall.month).padStart(2, '0')}-${String(wall.day).padStart(2, '0')}`;
  } catch {
    return at.toISOString().slice(0, 10);
  }
}
