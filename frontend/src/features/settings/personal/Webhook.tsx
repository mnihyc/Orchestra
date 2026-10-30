/** A webhook of the person's own: its name, the address it posts to and an optional secret that signs what it posts.
 *
 * The one way out a person sets up themselves. Orch posts each message to the address; with a secret each
 * is signed, so that whoever receives it can tell it came from here. A secret is write-only: once set, the
 * field says so, and leaving it empty keeps it. What the server finds wrong is shown at the field it names.
 */
import { useMutation } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { createOwnWebhook, updateChannel } from '@/api/notifications';
import type { NotificationChannel } from '@/generated/api-types';
import { useActKey, useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input, PasswordInput } from '@/ui/Input';
import { toast } from '@/ui/Toast';

/** The most characters of a name, as the server holds it. */
const NAME_MOST = 80;

/** Whether a text is a web address a message could be posted to. */
export function isWebAddress(text: string): boolean {
  try {
    const url = new URL(text.trim());
    return url.protocol === 'https:' || url.protocol === 'http:';
  } catch {
    return false;
  }
}

interface Values extends Record<string, string> {
  name: string;
  'config.url': string;
  'secrets.secret': string;
}

export function WebhookDialog({ channel, open, onOpenChange }: { channel: NotificationChannel | null; open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const held = (channel?.config ?? {}) as { url?: unknown };
  const secretHeld = channel?.secrets_set.includes('secret') === true;
  const form = useForm<Values>(
    { name: channel?.name ?? '', 'config.url': typeof held.url === 'string' ? held.url : '', 'secrets.secret': '' },
    {
      name: (value) => (value.trim() === '' ? t('notifications.webhook.nameNeeded') : value.trim().length > NAME_MOST ? t('notifications.webhook.nameLong', { most: NAME_MOST }) : null),
      'config.url': (value) => (isWebAddress(value) ? null : t('notifications.webhook.addressWrong')),
    },
  );
  const save = useMutation({
    mutationFn: (values: Values) => {
      const secret = values['secrets.secret'];
      if (channel === null) {
        return createOwnWebhook(
          { name: values.name.trim(), config: { url: values['config.url'].trim() }, ...(secret === '' ? {} : { secrets: { secret } }) },
          actKey(values),
        );
      }
      return updateChannel(channel.id, { name: values.name.trim(), config: { url: values['config.url'].trim() }, ...(secret === '' ? {} : { secrets: { secret } }) });
    },
    onSuccess: (saved) => {
      toast({ tone: 'success', title: t(channel === null ? 'notifications.webhook.added' : 'notifications.webhook.saved', { name: saved.name }) });
      onOpenChange(false);
    },
    onError: (error) => {
      form.refuse(error);
    },
  });

  const name = form.field('name');
  const address = form.field('config.url');
  const secret = form.field('secrets.secret');
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      dismissible={!save.isPending}
      title={channel === null ? t('notifications.webhook.addTitle') : t('notifications.webhook.editTitle', { name: channel.name })}
      description={t('notifications.webhook.lead')}
      footer={
        <>
          <Button variant="ghost" disabled={save.isPending} onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="webhook-form" pending={save.isPending}>
            {channel === null ? t('notifications.webhook.addConfirm') : t('notifications.webhook.saveConfirm')}
          </Button>
        </>
      }
    >
      <form id="webhook-form" noValidate className="flex flex-col gap-4" onSubmit={form.submit((values) => save.mutate(values))}>
        <Input label={t('notifications.webhook.name')} autoComplete="off" {...name} />
        <Input label={t('notifications.webhook.address')} hint={t('notifications.webhook.addressHint')} type="url" autoComplete="off" inputMode="url" {...address} />
        <PasswordInput
          label={t('notifications.webhook.secret')}
          optional
          hint={secretHeld ? t('notifications.webhook.secretKept') : t('notifications.webhook.secretHint')}
          autoComplete="off"
          {...secret}
        />
        {save.isError && name.error === undefined && address.error === undefined && secret.error === undefined && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}
