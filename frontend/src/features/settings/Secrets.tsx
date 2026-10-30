/** The secrets of a place: their names and where each is set, written and replaced and removed, and never shown.
 *
 * A secret is given to the environments made under the place, sealed to their machines. The server
 * keeps its value sealed and gives no request that returns it, so this page cannot show it: it lists
 * names, says where each comes from and when it was last written, takes a new value (hidden as it is
 * typed and forgotten the moment it is sent), and takes a name away. A secret set in a place above is
 * listed and left alone.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { KeyRound, Plus, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { deleteSecret, putSecret, secretsQuery, type SettingsTarget } from '@/api/settings';
import { takes } from '@/features/settings/refusals';
import type { RowPlace } from '@/features/settings/SettingRow';
import type { SecretInfo } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { boxClasses } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input, PasswordInput } from '@/ui/Input';
import { PermissionGate } from '@/ui/PermissionGate';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

/** What the name of a secret looks like: capitals, digits and underscores, not beginning with a digit, up to 64 of them. */
export const SECRET_NAME = /^[A-Z_][A-Z0-9_]{0,63}$/;

/** Writes a secret: a new name and its value, or a new value for a name that exists. The value is held only until it is sent. */
function WriteSecret({ target, place, name: given, onClose }: { target: SettingsTarget; place: RowPlace; name: string | null; onClose: () => void }) {
  const { t } = useTranslation();
  const [name, setName] = useState(given ?? '');
  const [value, setValue] = useState('');
  const [sent, setSent] = useState(false);
  const nameWrong = given === null && !SECRET_NAME.test(name) ? t('settings.secrets.nameRule') : undefined;
  const valueWrong = value === '' ? t('settings.secrets.valueMissing') : undefined;
  const save = useMutation({
    mutationFn: () => putSecret(target, name, value),
    onSuccess: () => {
      // What was typed is not kept a moment longer than it takes to send.
      setValue('');
      toast({ tone: 'success', title: t('settings.secrets.written', { name }), description: t('settings.secrets.neverShown') });
      onClose();
    },
  });
  const submit = () => {
    setSent(true);
    if (nameWrong === undefined && valueWrong === undefined && !save.isPending) save.mutate();
  };
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !save.isPending && onClose()}
      title={given === null ? t('settings.secrets.newTitle', { place: place.name }) : t('settings.secrets.replaceTitle', { name: given })}
      description={t('settings.secrets.writeLead')}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={save.isPending} onClick={submit}>
            {given === null ? t('settings.secrets.write') : t('settings.secrets.replace')}
          </Button>
        </>
      }
    >
      <form
        noValidate
        className="flex flex-col gap-4 py-2"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        {given === null && (
          <Input label={t('settings.secrets.name')} hint={t('settings.secrets.nameHint')} mono value={name} error={sent ? nameWrong : undefined} onChange={(event) => setName(event.target.value.toUpperCase())} autoComplete="off" />
        )}
        <PasswordInput
          label={t('settings.secrets.value')}
          hint={t('settings.secrets.valueHint')}
          value={value}
          error={sent ? valueWrong : undefined}
          onChange={(event) => setValue(event.target.value)}
          autoComplete="off"
        />
        {save.error !== null && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}

export function Secrets({ target, place }: { target: SettingsTarget; place: RowPlace }) {
  const { t } = useTranslation();
  const list = useQuery(secretsQuery(target));
  const [writing, setWriting] = useState<{ name: string | null } | null>(null);
  const [removing, setRemoving] = useState<SecretInfo | null>(null);
  const may = holds(place.access, 'settings.manage');
  const hereId = target.kind === 'system' ? null : target.id;
  const remove = useMutation({
    mutationFn: (secret: SecretInfo) => deleteSecret(target, secret.name),
    onSuccess: (_none, secret) => {
      toast({ tone: 'success', title: t('settings.secrets.removed', { name: secret.name }) });
      setRemoving(null);
    },
  });
  return (
    <section aria-labelledby="settings-secrets" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 flex-col">
          <h3 id="settings-secrets" className="text-base font-medium text-text">
            {t('settings.secrets.title')}
          </h3>
          <p className="text-sm text-text-muted">{t('settings.secrets.lead')}</p>
        </div>
        <PermissionGate allowed={may} mode="block" reason={takes(t, 'settings.manage', place.name, place.owner)}>
          <Button size="sm" icon={<Plus aria-hidden />} onClick={() => setWriting({ name: null })}>
            {t('settings.secrets.add')}
          </Button>
        </PermissionGate>
      </div>
      {list.isPending ? (
        <SkeletonRows rows={2} />
      ) : list.isError ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : list.data.length === 0 ? (
        <EmptyState compact title={t('settings.secrets.none')} />
      ) : (
        <ul className={`${boxClasses} divide-y divide-border`}>
          {list.data.map((secret) => {
            const here = secret.node !== null && secret.node.id === hereId;
            return (
              <li key={secret.name} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2">
                <KeyRound className="size-4 shrink-0 text-text-subtle" aria-hidden />
                <span className="min-w-0 flex-1 basis-[min(100%,9rem)]">
                  <span className="font-mono text-base text-text">{secret.name}</span>
                  <span className="ml-3 text-sm text-text-muted">
                    {here ? t('settings.secrets.setHere') : t('settings.secrets.setIn', { place: secret.node?.name ?? t('settings.secrets.environment') })}
                  </span>
                </span>
                <span className="text-sm text-text-muted">
                  {secret.rotated_at === null ? t('settings.secrets.neverWritten') : <>{t('settings.secrets.writtenAt')} <RelativeTime at={secret.rotated_at} /></>}
                </span>
                <span className="text-sm text-text-subtle">{t('settings.secrets.hidden')}</span>
                {may && here && (
                  <>
                    <Button size="sm" variant="ghost" onClick={() => setWriting({ name: secret.name })} aria-label={t('settings.secrets.replaceNamed', { name: secret.name })}>
                      {t('settings.secrets.replace')}
                    </Button>
                    <IconButton size="sm" label={t('settings.secrets.removeNamed', { name: secret.name })} icon={<Trash2 aria-hidden />} onClick={() => setRemoving(secret)} />
                  </>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {writing !== null && <WriteSecret target={target} place={place} name={writing.name} onClose={() => setWriting(null)} />}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(next) => !next && setRemoving(null)}
        title={t('settings.secrets.removeTitle', { name: removing?.name ?? '' })}
        confirmLabel={t('settings.secrets.removeConfirm', { name: removing?.name ?? '' })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => removing !== null && remove.mutate(removing)}
      >
        {t('settings.secrets.removeBody')}
      </ConfirmDialog>
    </section>
  );
}
