/** The notice every page of the system shows across its top: its text, its level, and how long it stays, put up or taken down from here.
 *
 * It is the setting `system.banner`; what is saved is what the shell of every page shows, so the
 * form shows the notice as it will look before it is saved. Pressing the button shows it as under
 * way, and nothing is shown as put up before the server has said so.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { clearSystemSetting, setSystemSetting, systemSettingsQuery } from '@/api/admin';
import type { ResolvedSetting } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { boxClasses } from '@/shell/Page';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

const KEY = 'system.banner';

type Level = 'info' | 'warning';

interface Notice {
  text: string;
  level: Level;
  until: string | null;
}

/** The notice a setting holds, or `null` when it holds none or something that is no notice. */
export function noticeOf(value: unknown): Notice | null {
  if (typeof value !== 'object' || value === null) return null;
  const { text, level, until } = value as Record<string, unknown>;
  if (typeof text !== 'string') return null;
  return { text, level: level === 'warning' ? 'warning' : 'info', until: typeof until === 'string' ? until : null };
}

/** How long a notice stays, in the words of the choice, and in hours: none means until it is taken down. */
const STAYS = [
  { id: 'down', hours: null },
  { id: 'hour', hours: 1 },
  { id: 'day', hours: 24 },
  { id: 'week', hours: 24 * 7 },
] as const;
type Stay = (typeof STAYS)[number]['id'] | 'kept';

const HOUR_MS = 3_600_000;

function Form({ setting }: { setting: ResolvedSetting | undefined }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const held = noticeOf(setting?.value);
  const [text, setText] = useState(held?.text ?? '');
  const [level, setLevel] = useState<Level>(held?.level ?? 'info');
  const [stay, setStay] = useState<Stay>(held?.until === undefined || held.until === null ? 'down' : 'kept');
  const [taking, setTaking] = useState(false);

  const put = useMutation({
    mutationFn: (until: string | null) => setSystemSetting(KEY, { text: text.trim(), level, until }),
    onSuccess: () => {
      toast({ tone: 'success', title: t('admin.overview.banner.putUp') });
    },
  });

  const take = useMutation({
    mutationFn: () => clearSystemSetting(KEY),
    onSuccess: () => {
      setText('');
      setLevel('info');
      setStay('down');
      setTaking(false);
      toast({ tone: 'success', title: t('admin.overview.banner.takenDown') });
    },
  });

  const options = [
    ...(held?.until !== undefined && held.until !== null ? [{ value: 'kept' as const, ...choiceOption(t, 'bannerStay.kept', { time: formatDateTime(held.until, { locale, timeZone }) }) }] : []),
    ...STAYS.map((one) => ({ value: one.id, ...choiceOption(t, `bannerStay.${one.id}`) })),
  ];
  const typed = text.trim();
  const unchanged = held !== null && held.text === typed && held.level === level && (stay === 'kept' || (stay === 'down' && held.until === null));

  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault();
        if (typed === '') return;
        // How long it stays is counted from the moment of the press, and the server is told the end.
        const hours = STAYS.find((one) => one.id === stay)?.hours ?? null;
        put.mutate(stay === 'kept' ? (held?.until ?? null) : hours === null ? null : new Date(Date.now() + hours * HOUR_MS).toISOString());
      }}
    >
      {typed !== '' && (
        <div className="flex flex-col gap-1">
          <p className="text-sm font-medium text-text">{t('admin.overview.banner.preview')}</p>
          <Banner tone={level} className="rounded-md border">
            {typed}
          </Banner>
        </div>
      )}
      <Textarea label={t('admin.overview.banner.text')} value={text} rows={2} onChange={(event) => setText(event.target.value)} hint={t('admin.overview.banner.textHint')} />
      <div className="flex flex-wrap items-end gap-4">
        <div className="flex flex-col gap-1">
          <span className="text-sm font-medium text-text">{t('admin.overview.banner.level')}</span>
          <SegmentedControl
            label={t('admin.overview.banner.level')}
            value={level}
            onValueChange={setLevel}
            segments={[
              { value: 'info', ...choiceOption(t, 'severity.info') },
              { value: 'warning', ...choiceOption(t, 'severity.warning') },
            ]}
          />
        </div>
        <Select label={t('admin.overview.banner.stays')} value={stay} onValueChange={setStay} options={options} fieldClassName="min-w-[12rem]" />
      </div>
      {(put.error !== null || take.error !== null) && <ErrorAlert error={put.error ?? take.error} />}
      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" variant="primary" pending={put.isPending} disabled={typed === '' || unchanged}>
          {held === null ? t('admin.overview.banner.putUpButton') : t('admin.overview.banner.save')}
        </Button>
        {held !== null && (
          <Button variant="danger-ghost" onClick={() => setTaking(true)}>
            {t('admin.overview.banner.takeDown')}
          </Button>
        )}
      </div>
      <ConfirmDialog
        open={taking}
        onOpenChange={setTaking}
        title={t('admin.overview.banner.takeDownTitle')}
        confirmLabel={t('admin.overview.banner.takeDown')}
        danger
        pending={take.isPending}
        error={take.error ?? undefined}
        onConfirm={() => take.mutate()}
      >
        {t('admin.overview.banner.takeDownBody')}
      </ConfirmDialog>
    </form>
  );
}

export function BannerEditor() {
  const settings = useQuery(systemSettingsQuery);
  const setting = settings.data?.find((one) => one.key === KEY);
  if (settings.isPending) return <SkeletonRows rows={3} />;
  if (settings.isError) return <ErrorState compact error={settings.error} retrying={settings.isRefetching} onRetry={() => void settings.refetch()} />;
  return (
    <div className={cx(boxClasses, 'p-4')}>
      {/* A new form for each notice the server holds, so that what it says is what the form starts from. */}
      <Form key={JSON.stringify(setting?.value ?? null)} setting={setting} />
    </div>
  );
}
