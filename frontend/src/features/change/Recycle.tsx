/** Adjustable threshold and exact dry-run consequences: what is idle at a host or a place, listed before it is stopped.
 *
 * Recycling makes room by stopping the environments nobody is using. The person sets how long an environment has been idle
 * for it to count (the dialog begins at the hour the settings give) and whether those with processes running count too;
 * every change of either asks the system again, as a dry run that stops nothing, so that what is on the screen is exactly
 * what will stop. The sentence says what that comes to ("Their files stay; each starts again when a message arrives"),
 * the rest are counted with why they are left: something runs in them, or the person may not stop them.
 */
import { keepPreviousData, useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { recycleHost, recycleNode } from '@/api/changes';
import type { FleetEntry, RecycleBody } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatBytes } from '@/lib/format';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

/** Where the dialog begins: the hour the settings give for an environment to count as idle. */
const START_IDLE_MINUTES = 60;
/** How long the threshold rests before the list is asked for again. */
const ASK_PAUSE_MS = 300;

export interface RecycleProps {
  /** A host, for everything that runs on it, or a place, for the environments that lie in it. */
  scope: { kind: 'host' | 'node'; id: string; name: string };
  open: boolean;
  onClose: () => void;
}

export function Recycle({ scope, open, onClose }: RecycleProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const [minutes, setMinutes] = useState(START_IDLE_MINUTES);
  const [withProcesses, setWithProcesses] = useState(false);
  const rested = useDebounced(minutes, ASK_PAUSE_MS);
  const ask = (dry: boolean): RecycleBody => ({ dry_run: dry, idle_minutes: rested, include_with_processes: withProcesses });
  const call = (body: RecycleBody) => (scope.kind === 'host' ? recycleHost(scope.id, body) : recycleNode(scope.id, body));

  const dry = useQuery({
    queryKey: ['recycle-dry-run', scope.kind, scope.id, rested, withProcesses],
    queryFn: () => call(ask(true)),
    staleTime: 0,
    gcTime: 30_000,
    retry: false,
    placeholderData: keepPreviousData,
  });
  const reply = dry.data;
  const stale = dry.isPlaceholderData || dry.isFetching || rested !== minutes;

  const stop = useMutation({
    mutationFn: () => call(ask(false)),
    onSuccess: (done) => {
      toast({ tone: 'success', title: t('change.recycle.stopped', { count: done.stopped }) });
    },
  });
  const done = stop.data;
  const count = reply?.candidates.length ?? 0;
  const freed = (reply?.candidates ?? []).reduce((sum, entry) => sum + (entry.memory_bytes ?? 0), 0);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !stop.isPending && onClose()}
      size="md"
      title={t('change.recycle.title', { name: scope.name })}
      description={t('change.recycle.about')}
      dismissible={!stop.isPending}
      footer={
        done !== undefined ? (
          <Button variant="primary" onClick={onClose}>
            {t('common.close')}
          </Button>
        ) : (
          <>
            <Button disabled={stop.isPending} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" pending={stop.isPending} blocked={reply === undefined || stale || count === 0} onClick={() => !stale && count > 0 && stop.mutate()} data-confirm>
              {t('change.recycle.confirm', { count })}
            </Button>
          </>
        )
      }
    >
      <div className="flex flex-col gap-4 py-2" data-recycle>
        {done !== undefined ? (
          <InlineAlert tone="success" title={t('change.recycle.stopped', { count: done.stopped })}>
            {t('change.recycle.stoppedAbout')}
          </InlineAlert>
        ) : (
          <>
            <div className="flex flex-wrap items-end gap-4">
              <Input
                label={t('change.recycle.minutes')}
                type="number"
                min={0}
                inputMode="numeric"
                value={String(minutes)}
                onChange={(event) => setMinutes(Math.max(0, Number(event.target.value) || 0))}
                fieldClassName="w-(--field-narrow)"
              />
              <Checkbox label={t('change.recycle.withProcesses')} hint={t('change.recycle.withProcessesHint')} checked={withProcesses} onCheckedChange={setWithProcesses} />
            </div>
            {dry.isError && <ErrorAlert error={dry.error} action={<Button size="sm" onClick={() => void dry.refetch()}>{t('common.tryAgain')}</Button>} />}
            {dry.isPending && <SkeletonRows rows={3} />}
            {reply !== undefined && (
              <div className={stale ? 'flex flex-col gap-3 opacity-60 transition-opacity' : 'flex flex-col gap-3'} aria-busy={stale} data-recycle-list>
                <p className="text-base text-text" data-recycle-sentence>
                  {count === 0 ? t('change.recycle.nothing') : t('change.recycle.sentence', { count })}
                  {count > 0 && freed > 0 && <span className="ml-1 text-text-muted">{t('change.recycle.freed', { size: formatBytes(freed, format.locale) })}</span>}
                </p>
                {count > 0 && (
                  <ul aria-label={t('change.recycle.list')} className="flex flex-col gap-1 rounded-lg border border-border bg-surface p-2">
                    {reply.candidates.map((entry) => (
                      <Candidate key={entry.environment.id} entry={entry} />
                    ))}
                  </ul>
                )}
                {(reply.skipped_active > 0 || reply.skipped_no_permission > 0) && (
                  <ul className="flex flex-col gap-1 text-sm text-text-muted" data-recycle-left>
                    {reply.skipped_active > 0 && <li>{t('change.recycle.leftActive', { count: reply.skipped_active })}</li>}
                    {reply.skipped_no_permission > 0 && <li>{t('change.recycle.leftNoPermission', { count: reply.skipped_no_permission })}</li>}
                  </ul>
                )}
              </div>
            )}
            {stop.error !== null && <ErrorAlert error={stop.error} />}
          </>
        )}
      </div>
    </Dialog>
  );
}

function Candidate({ entry }: { entry: FleetEntry }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  return (
    <li data-recycle-candidate={entry.environment.id} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 border-b border-border p-2 last:border-b-0">
      <span className="font-medium text-text">{entry.environment.name}</span>
      <span className="text-sm text-text-muted">{t('change.recycle.idle', { minutes: entry.idle_minutes ?? 0 })}</span>
      <span className="text-sm text-text-muted">{t('change.recycle.threads', { count: entry.threads })}</span>
      {entry.memory_bytes !== null && <span className="text-sm text-text-muted">{formatBytes(entry.memory_bytes, format.locale)}</span>}
    </li>
  );
}
