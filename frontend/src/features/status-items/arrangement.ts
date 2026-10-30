/** Status visibility, place and order are one presentation model, stored with the workbench. */
import { STATUS_ITEMS, STATUS_PLACES, type StatusItemId, type StatusPlace } from './registry';

export interface StatusEntry { id: StatusItemId; place: StatusPlace; shown: boolean }
export type StatusArrangement = readonly StatusEntry[];
export const defaultStatus = (): StatusEntry[] => STATUS_ITEMS.map(({ id, place, shown }) => ({ id, place, shown }));
export function readStatus(value: unknown): StatusEntry[] {
  if (!Array.isArray(value)) return defaultStatus();
  const entries: StatusEntry[] = [];
  for (const entry of value) {
    if (typeof entry !== 'object' || entry === null) continue;
    const data = entry as Record<string, unknown>;
    const item = STATUS_ITEMS.find((item) => item.id === data.id);
    if (item === undefined || entries.some((held) => held.id === item.id) || !STATUS_PLACES.some((place) => place === data.place) || !item.places.includes(data.place as StatusPlace) || typeof data.shown !== 'boolean') continue;
    entries.push({ id: item.id, place: data.place as StatusPlace, shown: data.shown });
  }
  return [...entries, ...defaultStatus().filter((entry) => !entries.some((held) => held.id === entry.id))];
}
export function showStatus(entries: StatusArrangement, id: StatusItemId, place: StatusPlace, shown: boolean): StatusEntry[] {
  if (entries.some((entry) => entry.id === id && entry.place !== place)) return moveStatus(entries, id, place).map((entry) => entry.id === id ? { ...entry, shown } : entry);
  return entries.map((entry) => entry.id === id ? { ...entry, place, shown } : entry);
}
export function moveStatus(entries: StatusArrangement, id: StatusItemId, place: StatusPlace): StatusEntry[] {
  const entry = entries.find((entry) => entry.id === id);
  return entry === undefined ? [...entries] : [...entries.filter((entry) => entry.id !== id), { ...entry, place, shown: true }];
}
export function orderStatus(entries: StatusArrangement, id: StatusItemId, before: StatusItemId): StatusEntry[] {
  const from = entries.find((entry) => entry.id === id), to = entries.find((entry) => entry.id === before);
  if (from === undefined || to?.place !== from.place || id === before) return [...entries];
  const next = entries.filter((entry) => entry.id !== id), index = next.findIndex((entry) => entry.id === before);
  next.splice(index, 0, from); return next;
}
export function stepStatus(entries: StatusArrangement, id: StatusItemId, direction: -1 | 1): StatusEntry[] {
  const entry = entries.find((entry) => entry.id === id);
  if (entry === undefined) return [...entries];
  const rows = entries.filter((held) => held.place === entry.place), at = rows.findIndex((held) => held.id === id), target = rows[at + direction];
  if (target === undefined) return [...entries];
  const next = [...entries], from = next.findIndex((held) => held.id === id), to = next.findIndex((held) => held.id === target.id);
  [next[from], next[to]] = [target, entry]; return next;
}
