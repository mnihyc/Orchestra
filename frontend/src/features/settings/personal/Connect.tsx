/** The dialog that connects a person's chat to a bot: a link, the same link as a QR code, and the wait for Start.
 *
 * The link opens the bot with a code and is good for ten minutes. Nothing is typed on either side, no number
 * and no code. When the person presses Start in the bot, their route is linked and their own feed says so:
 * the card reads "Connected as @name" and this dialog goes with the need it answered. A link that ran out is
 * made again on request.
 */
import { useMutation } from '@tanstack/react-query';
import { ExternalLink, LoaderCircle } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { linkRoute } from '@/api/notifications';
import { onUserEvent } from '@/realtime/stores/user';
import type { NotificationChannel } from '@/generated/api-types';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { QrCode } from '@/ui/QrCode';
import { TimeOfDay } from '@/ui/TimeOfDay';

/** The name of the bot a link opens, as `@name`: the first part of the path of a Telegram link. */
export function botOf(url: string): string | null {
  const found = /^https?:\/\/(?:t|telegram)\.me\/([A-Za-z0-9_]{3,})/.exec(url);
  return found?.[1] === undefined ? null : `@${found[1]}`;
}

export function ConnectDialog({
  channel,
  name,
  open,
  onOpenChange,
  onLinked,
}: {
  channel: NotificationChannel;
  /** What the bot is called: "Telegram". */
  name: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Asks the page to read the channels again, to find out whether Start was pressed. */
  onLinked: () => void;
}) {
  const { t } = useTranslation();
  const now = useNow();
  const link = useMutation({ mutationFn: () => linkRoute(channel.id) });
  const { mutate } = link;

  useEffect(() => {
    if (open) mutate();
  }, [open, mutate, channel.id]);

  useEffect(() => {
    if (!open || link.data === undefined) return;
    return onUserEvent('route', (data) => { if (data.channel_id === channel.id) onLinked(); });
  }, [open, link.data, onLinked, channel.id]);

  const made = link.data;
  const ranOut = made !== undefined && Date.parse(made.expires_at) <= now;
  const bot = made === undefined ? null : botOf(made.url);

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t('notifications.connect.title', { name })}
      description={bot === null ? t('notifications.connect.leadPlain', { name }) : t('notifications.connect.lead', { bot })}
      footer={
        <Button variant="ghost" onClick={() => onOpenChange(false)}>
          {t('common.close')}
        </Button>
      }
    >
      <div className="flex flex-col gap-4">
        {link.isPending && <LoaderCircle className="size-4 animate-spin text-text-muted" aria-label={t('common.loading')} />}
        {link.isError && <ErrorAlert error={link.error} action={<Button size="sm" onClick={() => mutate()}>{t('common.tryAgain')}</Button>} />}
        {made !== undefined && !ranOut && (
          <>
            <div className="flex flex-wrap items-start gap-4">
              <QrCode value={made.url} label={t('notifications.connect.qr')} className="size-(--field-narrow)" />
              <div className="flex min-w-(--field-wide) flex-1 flex-col gap-3">
                <div className="flex items-end gap-1">
                  <Input label={t('notifications.connect.link')} readOnly value={made.url} mono fieldClassName="min-w-0 flex-1" onFocus={(event) => event.currentTarget.select()} />
                  <CopyButton value={made.url} label={t('notifications.connect.copy')} />
                </div>
                <div>
                  <Button asChild variant="secondary" icon={<ExternalLink aria-hidden />}>
                    <a href={made.url} target="_blank" rel="noopener noreferrer">
                      {t('notifications.connect.open')}
                    </a>
                  </Button>
                </div>
              </div>
            </div>
            <p role="status" className="flex items-start gap-2 text-sm text-text-muted">
              <LoaderCircle className="mt-0.5 size-4 shrink-0 animate-spin" aria-hidden />
              <span className="min-w-0">
                {t('notifications.connect.waiting')} {t('notifications.connect.goodUntil')} <TimeOfDay at={made.expires_at} />
              </span>
            </p>
          </>
        )}
        {ranOut && (
          <div className="flex flex-col items-start gap-2">
            <p className="text-sm text-text-muted">{t('notifications.connect.ranOut')}</p>
            <Button variant="secondary" pending={link.isPending} onClick={() => mutate()}>
              {t('notifications.connect.again')}
            </Button>
          </div>
        )}
      </div>
    </Dialog>
  );
}
