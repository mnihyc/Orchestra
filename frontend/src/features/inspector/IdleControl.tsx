/** In-place timeout, provenance and keep awake.
 *
 * An environment that Orch made stops itself after a time without anybody using it. The time is shown as a fact
 * where it is read, "Stops when idle at 14:02", and is changed there, by whoever manages the environment, from a
 * short list: an hour, a day, three days, a week, or never. "Keep awake" starts the clock over.
 */
import { useMutation } from '@tanstack/react-query';
import { Moon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { api, ok } from '@/api/client';
import { keepEnvironmentAwake, type Door } from '@/api/workspace';
import { SettingHelpLink } from '@/features/help/SettingHint';
import type { Environment } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Menu, type MenuEntry } from '@/ui/Menu';

/** The values the menu offers, in minutes; 0 is never, as the setting says. */
export const IDLE_CHOICES = [
  { minutes: 60, key: 'hour' },
  { minutes: 1_440, key: 'day' },
  { minutes: 4_320, key: 'threeDays' },
  { minutes: 10_080, key: 'week' },
  { minutes: 0, key: 'never' },
] as const;

/** The setting that says how long an environment may stand idle before it is stopped. */
export const IDLE_KEY = 'env.idle_stop_minutes';

export function IdleControl({ environment, door, mayManage }: { environment: Environment; door: Door; mayManage: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const change = useMutation({
    mutationFn: (minutes: number) =>
      ok(
        api.PATCH('/environments/{id}', {
          params: { path: { id: environment.id } },
          body: { config: { ...(typeof environment.config === 'object' && environment.config !== null ? (environment.config as Record<string, unknown>) : {}), [IDLE_KEY]: minutes }, expected_revision: environment.revision },
        }),
      ),
  });
  const keep = useMutation({ mutationFn: () => keepEnvironmentAwake(environment.id, door) });
  const current = typeof environment.config === 'object' && environment.config !== null ? ((environment.config as Record<string, unknown>)[IDLE_KEY] as number | undefined) : undefined;
  const entries: MenuEntry[] = IDLE_CHOICES.map((choice) => ({
    id: String(choice.minutes),
    label: t(`inspector.environment.idle.${choice.key}`),
    disabled: mayManage ? false : t('inspector.environment.needsManage'),
    onSelect: () => change.mutate(choice.minutes),
  }));
  const text =
    environment.status !== 'running'
      ? t('inspector.environment.idle.notRunning')
      : typeof environment.idle_stop_at !== 'string'
        ? t('inspector.environment.idle.never')
        : t('inspector.environment.idle.stopsAt', { time: formatDateTime(environment.idle_stop_at, format) });
  return (
    <div className="flex flex-col gap-2" data-testid="idle-control">
      <div className="flex flex-wrap items-center gap-2">
        <Moon className="shrink-0 text-text-muted" aria-hidden />
        <span className="min-w-0 flex-1 text-sm text-text">{text}</span>
        <SettingHelpLink settingKey={IDLE_KEY} target={{ kind: 'environment', id: environment.id }} />
        <Menu
          label={t('inspector.environment.idle.menu')}
          trigger={
            <Button size="sm" pending={change.isPending}>
              {current === undefined ? t('inspector.environment.idle.change') : t('inspector.environment.idle.changeFrom', { value: t(`inspector.environment.idle.${IDLE_CHOICES.find((choice) => choice.minutes === current)?.key ?? 'day'}`) })}
            </Button>
          }
          entries={entries}
        />
        {environment.status === 'running' && mayManage && (
          <Button size="sm" pending={keep.isPending} onClick={() => keep.mutate()}>
            {t('inspector.environment.keepAwake')}
          </Button>
        )}
      </div>
      {change.error !== null && <ErrorAlert error={change.error} />}
      {keep.error !== null && <ErrorAlert error={keep.error} />}
    </div>
  );
}
