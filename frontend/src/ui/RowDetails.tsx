/** Shared row place, path and trailing time/actions, without squeezed partial labels. */
import type { ReactNode } from 'react';
import { Tooltip } from './Tooltip';
import './row-details.css';

export interface RowPlaceValue { name: string; path: string }
interface PathStep { id: string; name: string | null; redacted?: boolean }

/** A thing's own path ends at itself; documents and chats end at their container. */
export function nearestPlace(of: { id: string; path: readonly PathStep[]; place_label: string | null }): RowPlaceValue | null {
  const path = of.path.filter((part) => part.id !== of.id);
  const parent = path.at(-1);
  if (parent === undefined || parent.redacted || parent.name === null) return null;
  return { name: parent.name, path: of.place_label ?? path.map((part) => part.redacted || part.name === null ? '…' : part.name).join(' / ') };
}

/** The place yields before the name and disappears when fewer than six characters fit. */
export function RowPlace({ value, separator = false }: { value: RowPlaceValue | null; separator?: boolean }) {
  if (value === null) return null;
  return <span data-row-place>
    <Tooltip content={value.path}><span data-row-place-words className="text-sm text-text-muted"><span className="truncate">{value.name}</span>{separator && <span aria-hidden>·</span>}</span></Tooltip>
  </span>;
}

/** A path gives up its beginning first, retaining the nearest places and its complete tooltip. */
export function RowPath({ path }: { path: string }) {
  return <Tooltip content={path}><span data-row-path><bdi dir="ltr">{path}</bdi></span></Tooltip>;
}

/** Desktop actions replace the whole time; phones retain both in separate slots. */
export function RowEnd({ time, title, children }: { time: ReactNode; title?: string; children: ReactNode }) {
  return <span data-row-end title={title}>
    <span data-row-time className="text-sm text-text-muted tabular-nums">{time}</span>
    <span data-row-actions>{children}</span>
  </span>;
}
