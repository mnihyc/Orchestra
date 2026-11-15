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
import { formatBytes } from '@/lib/format';
import { MEMORY_BYTES_PER_MB, type ResourceSubject, resourceHint, resourceName } from './facts';
import ResourceCard from './ResourceCard';

const GLYPHS = { provider: PlugZap, model: Cpu, host: Server, environment: Box, mount: Box, mcp_server: Plug, template: Cpu, agent: Bot, skill: GraduationCap };
const chipClasses = 'resource-chip inline-flex min-w-0 max-w-full gap-1 rounded-md border bg-surface px-1.5 py-0.5 text-sm';

export function ResourceGlyph({ kind }: { kind: ResourceSubject['kind'] }) {
  const Icon = GLYPHS[kind];
  return <Icon aria-hidden className="size-4 shrink-0 text-text-muted" />;
}

/** A refused destination keeps what it has free visible before it can be chosen: what it measured, never what its environments declare. */
export function HostCapacity({ place }: { place: PlaceHost }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const used = place.used.memory_mb;
  if (place.capacity.memory_mb <= 0 || used === null) return null;
  const label = t('resourceFacts.memoryFree', {
    free: formatBytes(Math.max(0, place.capacity.memory_mb - used) * MEMORY_BYTES_PER_MB, locale),
    total: formatBytes(place.capacity.memory_mb * MEMORY_BYTES_PER_MB, locale),
  });
  return <span className="flex min-w-0 flex-col gap-1"><Meter neutral showActual look="compact" value={used / place.capacity.memory_mb * 100} label={label} /><span className="text-sm text-text-muted">{label}</span></span>;
}

export function ResourceHint({ subject, className, wrap = false }: { subject: ResourceSubject; className?: string; wrap?: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const now = useNow();
  const hint = resourceHint(subject, { ...format, t, now });
  return hint.text === '' ? null : <span data-resource-hint={subject.kind} data-warning={hint.warning || undefined}
    className={cx('block min-w-0 max-w-full text-sm font-normal', wrap ? 'whitespace-normal wrap-anywhere' : 'truncate', hint.warning ? 'text-warning-text' : 'text-text-muted', className)}>{hint.warning ? <StatusDot tone="warning" label={hint.text} showLabel className={cx('min-w-0 max-w-full', !wrap && '[&>span:last-child]:truncate')} /> : hint.text}</span>;
}

function previewResource(subject: ResourceSubject): CardPreview['resource'] {
  switch(subject.kind) {
    case 'provider': return {kind:subject.kind,id:subject.provider.id,data:subject.provider};
    case 'agent': return {kind:subject.kind,id:subject.agent.id,data:subject.agent};
    case 'environment': return {kind:subject.kind,id:subject.environment.id,data:subject.environment};
    case 'host': return {kind:subject.kind,id:subject.host.id,data:subject.place??subject.host};
    case 'mount': return {kind:subject.kind,id:subject.mount.id,data:subject.mount};
    case 'mcp_server': return {kind:subject.kind,id:subject.server.id,data:subject.server};
    case 'template': return {kind:subject.kind,id:subject.template.id,data:subject.template};
    case 'skill': return {kind:subject.kind,id:subject.skill.id,data:subject.skill};
    case 'model': return undefined;
  }
}

export function resourcePreview(subject: ResourceSubject): CardPreview {
  return { resource: previewResource(subject), label: resourceName(subject), hint: <ResourceHint subject={subject} />, renderName: (name) => <ResourceChip subject={subject} card={false} hint={false}>{name}</ResourceChip>, content: () => <ResourceCard subject={subject} /> };
}

/** An option's hint wraps below its name; its inspection card describes the same permitted subject. */
export function resourceChoice(subject: ResourceSubject) {
  const preview=resourcePreview(subject);
  const resource=preview.resource;
  const own=resource?.data!==null&&typeof resource?.data==='object'&&Object.hasOwn(resource.data,'home');
  return {description:<ResourceHint subject={subject} wrap/>,preview,...(own?{resource}:{})};
}

/** Names keep their native link or control; their details appear on hover and focus. */
export function ResourceChip({ subject, children, asChild = false, card = true, hint = true, wrap = false, className }: { subject: ResourceSubject; children?: ReactNode; asChild?: boolean; card?: boolean; hint?: boolean; wrap?: boolean; className?: string }) {
  const valueClass = cx('min-w-0', wrap ? 'whitespace-normal wrap-anywhere' : 'truncate');
  const rowClasses = cx(chipClasses, wrap ? 'items-start' : 'items-center');
  const glyph = wrap ? <span className="inline-flex h-5 shrink-0 items-center"><ResourceGlyph kind={subject.kind} /></span> : <ResourceGlyph kind={subject.kind} />;
  if (!card) return <span className={cx(rowClasses, 'border-border', className)}>{glyph}<span className={valueClass}>{children ?? resourceName(subject)}</span>{hint && <ResourceHint subject={subject} wrap={wrap} />}</span>;
  const trigger = asChild && isValidElement<{ children?: ReactNode; className?: string }>(children)
    ? cloneElement(children, { className: cx(rowClasses, 'border-control-border', children.props.className, className) }, <>{glyph}{typeof children.props.children === 'string' ? <span className={valueClass}>{children.props.children}</span> : children.props.children}</>)
    : <>{glyph}<span className={valueClass}>{children ?? resourceName(subject)}</span>{hint && <ResourceHint subject={subject} wrap={wrap} />}</>;
  return <DetailCard {...resourcePreview(subject)} asChild={asChild} className={cx(rowClasses, 'border-control-border', className)}>
    {trigger}
  </DetailCard>;
}

/** Existing name renderers use the same chip while their call sites adopt its explicit name. */
export const ResourceName = ResourceChip;
