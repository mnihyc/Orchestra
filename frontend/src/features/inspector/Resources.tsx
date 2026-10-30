/** Resource history against limits.
 *
 * What the environment as a whole used over the last hour: the processor, the memory, the disk and the
 * network, each as a small chart with its figure now and its highest, and the limit it has drawn as a
 * line where the environment has one. It is the answer to "is the agent slow, or is the environment
 * starved". The samples are asked for again every few seconds, and the live connection's, where it is served, are added as they come.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Gauge } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { processKeys, SAMPLE_MINUTES, samplesQuery } from '@/api/processes';
import type { Door } from '@/api/workspace';
import type { ResourceSample } from '@/generated/api-types';
import type { WorkspaceAccess } from '@/features/files/door';
import type { ViewProps } from '@/features/workbench/views';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { socket } from '@/realtime/live';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Sparkline } from '@/ui/Sparkline';

import { ThreadViewFrame } from './parts';

/** How many samples the live connection adds before the oldest are let go: an hour at one every five seconds. */
const SAMPLES_KEPT = (SAMPLE_MINUTES * 60) / 5;

function useStatsFeed(door: Door): void {
  const client = useQueryClient();
  useEffect(() => {
    if (door.threadId === null) return;
    const subscription = socket.subscribe(`env:${door.environmentId}:stats`, {
      since: () => null,
      onFrame: (frame) => {
        if (frame.op !== 'event' || typeof frame.data.at !== 'string') return;
        const sample = frame.data as unknown as ResourceSample;
        client.setQueryData<ResourceSample[]>(processKeys.samples(door, SAMPLE_MINUTES), (before) => [...(before ?? []), sample].slice(-SAMPLES_KEPT));
      },
    });
    return () => subscription.unsubscribe();
  }, [client, door]);
}

function Chart({ title, now, peak, limit, label, series, max }: { title: string; now: string; peak: string; limit: string | null; label: string; series: { values: number[]; tone?: 'accent' | 'info' }[]; max?: number }) {
  const { t } = useTranslation();
  return (
    <section aria-label={title} className="flex flex-col gap-1 px-3 py-2">
      <div className="flex flex-wrap items-baseline gap-x-2 text-sm">
        <h3 className="font-medium text-text">{title}</h3>
        <span className="tabular-nums text-text" data-testid={`now-${title}`}>
          {now}
        </span>
        <span className="text-xs text-text-muted">{t('inspector.resources.peak', { value: peak })}</span>
        {limit !== null && <span className="text-xs text-text-muted">{t('inspector.resources.limit', { value: limit })}</span>}
      </div>
      <Sparkline series={series} label={label} {...(max === undefined ? {} : { max })} limit={null} />
    </section>
  );
}

/** A measurement is never below nothing: the machine's own figures can come as `-0`, which a number format writes as "-0". */
const nonNegative = (value: number): number => (value > 0 ? value : 0);

function ResourcesPanel({ access }: { access: WorkspaceAccess }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const query = useQuery(samplesQuery(access.door));
  useStatsFeed(access.door);
  if (query.error !== null && query.data === undefined) return <ErrorState compact className="h-full" error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data === undefined) return <SkeletonRows rows={4} />;
  const samples = query.data.map((sample) => ({
    ...sample,
    cpu_percent: nonNegative(sample.cpu_percent),
    memory_bytes: nonNegative(sample.memory_bytes),
    disk_used_bytes: sample.disk_used_bytes === null ? null : nonNegative(sample.disk_used_bytes),
    net_rx_bps: nonNegative(sample.net_rx_bps),
    net_tx_bps: nonNegative(sample.net_tx_bps),
  }));
  const last = samples.at(-1);
  if (last === undefined) return <EmptyState compact className="h-full" icon={<Gauge aria-hidden />} title={t('inspector.resources.none')} description={t('inspector.resources.noneAbout')} />;
  const peakOf = (pick: (sample: ResourceSample) => number): number => Math.max(...samples.map(pick));
  const percent = (value: number) => `${formatNumber(value, locale, { maximumFractionDigits: 0 })}%`;
  const rate = (value: number) => `${formatBytes(value, locale)}/s`;
  const cpuLimit = last.cpu_limit_millis === null ? null : last.cpu_limit_millis / 10;
  return (
    <div className="h-full overflow-auto" data-testid="resources-panel">
      <Chart
        title={t('inspector.resources.cpu')}
        now={percent(last.cpu_percent)}
        peak={percent(peakOf((sample) => sample.cpu_percent))}
        limit={cpuLimit === null ? null : t('inspector.resources.cpus', { count: (last.cpu_limit_millis ?? 0) / 1000 })}
        label={t('inspector.resources.chartCpu')}
        series={[{ values: samples.map((sample) => sample.cpu_percent) }]}
        {...(cpuLimit === null ? {} : { max: Math.max(cpuLimit, 1) })}
      />
      <Chart
        title={t('inspector.resources.memory')}
        now={formatBytes(last.memory_bytes, locale)}
        peak={formatBytes(peakOf((sample) => sample.memory_bytes), locale)}
        limit={last.memory_limit_bytes === null ? null : formatBytes(last.memory_limit_bytes, locale)}
        label={t('inspector.resources.chartMemory')}
        series={[{ values: samples.map((sample) => sample.memory_bytes) }]}
        {...(last.memory_limit_bytes === null ? {} : { max: last.memory_limit_bytes })}
      />
      <Chart
        title={t('inspector.resources.disk')}
        now={last.disk_used_bytes === null ? t('inspector.resources.unknown') : formatBytes(last.disk_used_bytes, locale)}
        peak={formatBytes(Math.max(0, ...samples.map((sample) => sample.disk_used_bytes ?? 0)), locale)}
        limit={last.disk_limit_bytes === null ? null : formatBytes(last.disk_limit_bytes, locale)}
        label={t('inspector.resources.chartDisk')}
        series={[{ values: samples.map((sample) => sample.disk_used_bytes ?? 0) }]}
        {...(last.disk_limit_bytes === null ? {} : { max: last.disk_limit_bytes })}
      />
      <Chart
        title={t('inspector.resources.network')}
        now={t('inspector.resources.netNow', { down: rate(last.net_rx_bps), up: rate(last.net_tx_bps) })}
        peak={rate(Math.max(peakOf((sample) => sample.net_rx_bps), peakOf((sample) => sample.net_tx_bps)))}
        limit={null}
        label={t('inspector.resources.chartNetwork')}
        series={[{ values: samples.map((sample) => sample.net_rx_bps) }, { values: samples.map((sample) => sample.net_tx_bps), tone: 'info' }]}
      />
      <p className="px-3 pb-3 text-xs text-text-muted">{t('inspector.resources.span', { minutes: SAMPLE_MINUTES })}</p>
    </div>
  );
}

/** The Resources view of a thread. */
export function ResourcesView(_props: ViewProps) {
  return <ThreadViewFrame need="terminal.view" icon={<Gauge aria-hidden />}>{(access) => <ResourcesPanel access={access} />}</ThreadViewFrame>;
}
