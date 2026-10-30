/** Quiet resource hints and lazy cards, shared by chooser rows, named chips and status items. */
import { cloneElement, isValidElement, type ReactNode } from 'react';
import { Box, Bot, Cpu, GraduationCap, Plug, PlugZap, Server } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { cx } from '@/ui/cx';
import { DetailCard, type CardPreview } from '@/ui/DetailCard';
import { Meter } from '@/ui/Meter';
import { StatusDot } from '@/ui/StatusDot';
import type { PlaceHost } from '@/generated/api-types';
import { type ResourceSubject, resourceHint, resourceName } from './facts';
import ResourceCard from './ResourceCard';

const GLYPHS = { provider: PlugZap, model: Cpu, host: Server, environment: Box, mount: Box, mcp_server: Plug, template: Cpu, agent: Bot, skill: GraduationCap };
const chipClasses = 'resource-chip inline-flex min-w-0 max-w-full items-center gap-1 rounded-md border bg-surface px-1.5 py-0.5 text-sm';

export function ResourceGlyph({ kind }: { kind: ResourceSubject['kind'] }) {
  const Icon = GLYPHS[kind];
  return <Icon aria-hidden className="size-4 shrink-0 text-text-muted" />;
}

/** The places list has always used binary gigabytes for its declared-capacity caption. */
const MEMORY_MB_PER_GIB = 1024;

/** A refused destination keeps its already supplied capacity visible before it can be chosen. */
export function HostCapacity({ place }: { place: PlaceHost }) {
  const { t } = useTranslation();
  if (place.capacity.memory_mb <= 0) return null;
  const label = t('change.where.declared', { declared: Math.round(place.declared.memory_mb / MEMORY_MB_PER_GIB), capacity: Math.round(place.capacity.memory_mb / MEMORY_MB_PER_GIB) });
  return <span className="flex min-w-0 flex-col gap-1"><Meter neutral showActual look="compact" value={place.declared.memory_mb / place.capacity.memory_mb * 100} label={label} /><span className="text-sm text-text-muted">{label}</span></span>;
}

export function ResourceHint({ subject, className }: { subject: ResourceSubject; className?: string }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const now = useNow();
  const hint = resourceHint(subject, { ...format, t, now });
  return hint.text === '' ? null : <span data-resource-hint={subject.kind} data-warning={hint.warning || undefined}
    className={cx('block min-w-0 max-w-full truncate text-sm font-normal', hint.warning ? 'text-warning-text' : 'text-text-muted', className)}>{hint.warning ? <StatusDot tone="warning" label={hint.text} showLabel className="min-w-0 max-w-full [&>span:last-child]:truncate" /> : hint.text}</span>;
}

export function resourcePreview(subject: ResourceSubject): CardPreview {
  return { label: resourceName(subject), hint: <ResourceHint subject={subject} />, renderName: (name) => <ResourceChip subject={subject} card={false} hint={false}>{name}</ResourceChip>, content: () => <ResourceCard subject={subject} /> };
}

/** An option's one-line hint and its inspection card always describe the same permitted subject. */
export function resourceChoice(subject: ResourceSubject) {
  return { description: <ResourceHint subject={subject} />, preview: resourcePreview(subject) };
}

/** Names keep their native link or control; their details appear on hover and focus. */
export function ResourceChip({ subject, children, asChild = false, card = true, hint = true, className }: { subject: ResourceSubject; children?: ReactNode; asChild?: boolean; card?: boolean; hint?: boolean; className?: string }) {
  if (!card) return <span className={cx(chipClasses, 'border-border', className)}><ResourceGlyph kind={subject.kind} /><span className="min-w-0 truncate">{children ?? resourceName(subject)}</span>{hint && <ResourceHint subject={subject} className="truncate" />}</span>;
  const trigger = asChild && isValidElement<{ children?: ReactNode; className?: string }>(children)
    ? cloneElement(children, { className: cx(chipClasses, 'border-control-border', children.props.className, className) }, <><ResourceGlyph kind={subject.kind} />{typeof children.props.children === 'string' ? <span className="min-w-0 truncate">{children.props.children}</span> : children.props.children}</>)
    : <><ResourceGlyph kind={subject.kind} /><span className="min-w-0 truncate">{children ?? resourceName(subject)}</span>{hint && <ResourceHint subject={subject} className="truncate" />}</>;
  return <DetailCard {...resourcePreview(subject)} asChild={asChild} className={cx(chipClasses, 'border-control-border', className)}>
    {trigger}
  </DetailCard>;
}

/** Existing name renderers use the same chip while their call sites adopt its explicit name. */
export const ResourceName = ResourceChip;
