/** Notification channel forms shared by root orgs and the system: declared fields, an explicit test and its result.
 *
 * A channel is set up once by whoever makes the rules of the root org, and nothing goes through it to
 * anybody until each member switches their own route on in their personal settings: the section says
 * so. Each channel can be switched on and off, tried with Test (the far side's own answer is shown
 * when it fails), changed and removed, after saying that removing it ends every member's route through
 * it. A department has no channels of its own: its members are members of the root org, so its section
 * is one line that leads to the root's. Secrets are written through the same form and never read back.
 */
import { type QueryClient, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { FlaskConical, Mail, Pencil, Plus, Trash2, Webhook } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { metaQuery } from '@/api/auth';
import { channelsKey, channelsQuery, createOrgChannel, createSystemChannel, deleteChannel, notifiersQuery, testChannel, updateChannel } from '@/api/settings';
import { NotifierDialog, type NotifierFormValue } from '@/features/settings/NotifierForm';
import { takes } from '@/features/settings/refusals';
import { EnabledRoutes, enabledRouteCount } from '@/features/settings/personal/away/Manager';
import type { Notifier, NotificationChannel, NotificationTestReply, Org } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { boxClasses, Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { PermissionGate } from '@/ui/PermissionGate';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { toast } from '@/ui/Toast';
import { linkClasses } from '@/ui/link';

/** Whether a kind of channel is tried by sending to an address that is asked for: one that reaches people alone and is not linked by a chat. */
export function testNeedsAddress(notifier: Pick<Notifier, 'links' | 'targets'> | undefined): boolean {
  return notifier !== undefined && !notifier.links && !notifier.targets.includes('destination');
}

/** One sample sent now, and what the far side answered. */
export function TestDialog({
  title,
  needsAddress,
  run,
  onClose,
}: {
  title: string;
  needsAddress: boolean;
  run: (to: string) => Promise<NotificationTestReply>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [to, setTo] = useState('');
  const [result, setResult] = useState<NotificationTestReply | null>(null);
  const send = useMutation({ mutationFn: () => run(to.trim()), onSuccess: setResult });
  const wrong = needsAddress && to.trim() === '';
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !send.isPending && onClose()}
      title={title}
      description={t('settings.channels.testLead')}
      dismissible={!send.isPending}
      footer={
        <>
          <Button disabled={send.isPending} onClick={onClose}>
            {t('common.close')}
          </Button>
          <Button variant="primary" icon={<FlaskConical aria-hidden />} pending={send.isPending} blocked={wrong} onClick={() => send.mutate()}>
            {result === null ? t('settings.channels.sendTest') : t('settings.channels.sendTestAgain')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2" aria-live="polite">
        {needsAddress && <Input label={t('settings.channels.testTo')} hint={t('settings.channels.testToHint')} type="email" value={to} onChange={(event) => setTo(event.target.value)} autoComplete="off" />}
        {result !== null &&
          (result.ok ? (
            <InlineAlert tone="success">{t('away.outcome.accepted')}</InlineAlert>
          ) : (
            <InlineAlert tone="danger" title={t('settings.channels.testFailed')}>
              <p>{t('settings.channels.farSide')}</p>
              <pre data-scroll-allow="code" className="mt-1 overflow-x-auto rounded-sm bg-bg-muted px-2 py-1 font-mono text-sm wrap-anywhere whitespace-pre-wrap text-text">{result.error ?? t('settings.channels.farSideNothing')}</pre>
            </InlineAlert>
          ))}
        {send.error !== null && <ErrorAlert error={send.error} />}
      </div>
    </Dialog>
  );
}

type ChannelPlace = Pick<Org, 'id' | 'name'> & { owner: Org['owner'] | null };

/** The native answer replaces just this scope's channel; other scopes and write-only fields stay separate. */
function acceptChannel(cache: QueryClient, scope: string, channel: NotificationChannel) {
  cache.setQueryData<NotificationChannel[]>(channelsKey(scope), (held) => held === undefined ? held
    : held.some((entry) => entry.id === channel.id) ? held.map((entry) => entry.id === channel.id ? channel : entry) : [...held, channel]);
}

function ChannelCard({ channel, notifier, notifiers, org, may }: { channel: NotificationChannel; notifier: Notifier | undefined; notifiers: readonly Notifier[]; org: ChannelPlace; may: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [dialog, setDialog] = useState<'edit' | 'test' | 'remove' | null>(null);
  const done = () => void queryClient.invalidateQueries({ queryKey: channelsKey(org.id) });
  const toggle = useMutation({ mutationFn: (enabled: boolean) => updateChannel(channel.id, { enabled }), onSuccess: (channel) => acceptChannel(queryClient, org.id, channel) });
  const save = useMutation({
    mutationFn: (value: NotifierFormValue) =>
      updateChannel(channel.id, { name: value.name, config: value.config, ...(Object.keys(value.secrets).length > 0 ? { secrets: value.secrets } : {}) }),
    onSuccess: (channel) => {
      acceptChannel(queryClient, org.id, channel);
      toast({ tone: 'success', title: t('settings.channels.saved', { name: channel.name }) });
      setDialog(null);
    },
  });
  const remove = useMutation({
    mutationFn: () => deleteChannel(channel.id),
    onSuccess: () => {
      queryClient.setQueryData<NotificationChannel[]>(channelsKey(org.id), (held) => held?.filter((entry) => entry.id !== channel.id));
      toast({ tone: 'success', title: t('settings.channels.removed', { name: channel.name }) });
      setDialog(null);
    },
  });
  const Icon = channel.kind === 'webhook' ? Webhook : Mail;
  const enabled = toggle.isPending ? toggle.variables : channel.enabled;
  return (
    <li className="flex flex-col gap-3 px-4 py-4" data-channel={channel.id}>
      <div className="flex flex-wrap items-start gap-x-4 gap-y-2">
        <Icon className="mt-1 size-4 shrink-0 text-text-subtle" aria-hidden />
        <div className="min-w-0 flex-1 basis-[min(100%,14rem)]">
          <h3 className="text-base font-medium wrap-anywhere text-text">{channel.name}</h3>
          <p className="text-sm text-text-muted">{notifier?.title ?? channel.kind}</p>
        </div>
        <Switch
          label={<span className="text-sm">{t('day100.names.enableDelivery')}</span>}
          checked={enabled}
          disabled={!may || toggle.isPending}
          onCheckedChange={(next) => toggle.mutate(next)}
          className="items-center gap-3"
        />
      </div>
      {toggle.error !== null && <ErrorAlert error={toggle.error} />}
      <div className="flex flex-col gap-1 text-sm text-text-muted">
        <EnabledRoutes channel={channel} />
        {channel.last_ok_at !== null && (
          <p>
            {t('away.outcome.lastAccepted')} <RelativeTime at={channel.last_ok_at} />
          </p>
        )}
        {channel.last_ok_at === null && channel.last_error === null && <p>{t('settings.channels.neverSent')}</p>}
        {channel.last_error !== null && (
          <InlineAlert tone="danger" title={t('settings.channels.lastError')}>
            <span className="font-mono text-sm wrap-anywhere">{channel.last_error}</span>
          </InlineAlert>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" icon={<FlaskConical aria-hidden />} onClick={() => setDialog('test')} aria-label={t('settings.channels.testNamed', { name: channel.name })}>
          {t('settings.channels.test')}
        </Button>
        <PermissionGate allowed={may} mode="block" reason={takes(t, 'settings.manage', org.name, org.owner)}>
          <Button size="sm" icon={<Pencil aria-hidden />} onClick={() => setDialog('edit')} aria-label={t('settings.channels.editNamed', { name: channel.name })}>
            {t('settings.channels.edit')}
          </Button>
        </PermissionGate>
        {may && (
          <Button size="sm" variant="danger-ghost" icon={<Trash2 aria-hidden />} onClick={() => setDialog('remove')} aria-label={t('settings.channels.removeNamed', { name: channel.name })}>
            {t('settings.channels.remove')}
          </Button>
        )}
      </div>
      {dialog === 'test' && (
        <TestDialog
          title={t('settings.channels.testTitle', { name: channel.name })}
          needsAddress={testNeedsAddress(notifier)}
          run={(to) => testChannel(channel.id, to).catch((error: unknown) => { done(); throw error; })}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === 'edit' && (
        <NotifierDialog
          title={t('settings.channels.editTitle', { name: channel.name })}
          notifiers={notifiers}
          existing={{ kind: channel.kind, name: channel.name, config: channel.config, secretsSet: channel.secrets_set }}
          submitLabel={t('settings.save')}
          pending={save.isPending}
          error={save.error}
          onSubmit={(value) => save.mutate(value)}
          onClose={() => setDialog(null)}
        />
      )}
      <ConfirmDialog
        open={dialog === 'remove'}
        onOpenChange={(next) => !next && setDialog(null)}
        title={t('settings.channels.removeTitle', { name: channel.name })}
        confirmLabel={t('settings.channels.removeConfirm', { name: channel.name })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => remove.mutate()}
      >
        <p>{t('settings.channels.removeBody', { org: org.name })}</p>
        {enabledRouteCount(channel) !== null && <p className="mt-2">{t('away.manager.remove', { count:enabledRouteCount(channel) ?? 0 })}</p>}
        {notifier?.links === true && <p className="mt-2">{t('settings.channels.removeBot')}</p>}
      </ConfirmDialog>
    </li>
  );
}

export function Channels({ org, level = 2 }: { org: Org; level?: 2 | 3 }) {
  const { t } = useTranslation();
  const root = org.address.split('.')[0] ?? org.slug;
  if (org.parent_id !== null) return <Section id="channels" level={level} title={t('day100.names.toPeople')}>
    <p className={`${boxClasses} flex flex-wrap items-center gap-3 px-4 py-3 text-base text-text`}>
      {t('settings.channels.setForAll', { root: org.path[0]?.name ?? root })}
      <Link to="/org/$slug/settings/$section" params={{ slug: root, section: 'channels' }} className={linkClasses}>{t('settings.channels.open')}</Link>
    </p>
  </Section>;
  return <ManagedChannels org={org} may={holds(org.access, 'settings.manage')} level={level} />;
}

export function SystemChannels() {
  const { t } = useTranslation();
  return <ManagedChannels org={{ id: 'system', name: t('access.place.system'), owner: null }} may system embedded />;
}

function ManagedChannels({ org, may, level = 2, system = false, embedded = false }: { org: ChannelPlace; may: boolean; level?: 2 | 3; system?: boolean; embedded?: boolean }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const actKey = useActKey();
  const meta = useQuery({ ...metaQuery, enabled: !system });
  const refusal = takes(t, 'settings.manage', org.name, org.owner);
  const notifiers = useQuery(notifiersQuery);
  const list = useQuery({ ...channelsQuery(org.id), enabled: may });
  const [making, setMaking] = useState(false);
  const make = useMutation({
    mutationFn: (value: NotifierFormValue) => {
      const write = { name: value.name, config: value.config, ...(Object.keys(value.secrets).length > 0 ? { secrets: value.secrets } : {}) };
      return system ? createSystemChannel(value.kind, write, actKey(value)) : createOrgChannel(org.id, value.kind, write, actKey(value));
    },
    onSuccess: (channel) => {
      acceptChannel(cache, org.id, channel);
      toast({ tone: 'success', title: t('settings.channels.made', { name: channel.name }) });
      setMaking(false);
    },
  });

  const people = (notifiers.data ?? []).filter((entry) => entry.owners.includes(system ? 'system' : 'org') && entry.targets.includes('person'));
  const have = new Set((list.data ?? []).map((channel) => channel.kind));
  // A kind the org may have only once is not offered again.
  const offered = people.filter((entry) => !(entry.single && have.has(entry.name)));
  const mailHere = (list.data ?? []).some((channel) => channel.kind === 'email');

  const action = <PermissionGate allowed={may} mode="block" reason={refusal}>
          <Button size="sm" variant="primary" icon={<Plus aria-hidden />} blocked={offered.length === 0 || list.isError || notifiers.isError} onClick={() => setMaking(true)}>
            {t('settings.channels.add')}
          </Button>
        </PermissionGate>;
  const body = <>
      {!system && <InlineAlert tone="info">{t('settings.channels.sendsNothing')}</InlineAlert>}
      {!system && !mailHere && <p className="text-sm text-text-muted">{meta.data?.mail === true ? t('settings.channels.mailSystem') : t('settings.channels.mailNone', { name: org.name })}</p>}
      {!may ? (
        <EmptyState compact icon={<Mail aria-hidden />} title={t('settings.channels.readers', { name: org.name })} description={refusal} />
      ) : list.isPending || notifiers.isPending ? (
        <SkeletonRows rows={3} />
      ) : list.isError ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : notifiers.isError ? (
        <ErrorState compact error={notifiers.error} retrying={notifiers.isRefetching} onRetry={() => void notifiers.refetch()} />
      ) : list.data.length === 0 ? (
        <EmptyState compact icon={<Mail aria-hidden />} title={t('settings.channels.none', { name: org.name })} description={t('settings.channels.noneAbout')} />
      ) : (
        <ul className={`${boxClasses} divide-y divide-border`} aria-label={t('settings.channels.list', { name: org.name })}>
          {list.data.map((channel) => (
            <ChannelCard key={channel.id} channel={channel} notifier={people.find((entry) => entry.name === channel.kind)} notifiers={people} org={org} may={channel.can_edit} />
          ))}
        </ul>
      )}
      {making && (
        <NotifierDialog
          title={t('settings.channels.addTitle', { name: org.name })}
          lead={t('settings.channels.addLead')}
          notifiers={offered}
          submitLabel={t('settings.channels.make')}
          pending={make.isPending}
          error={make.error}
          onSubmit={(value) => make.mutate(value)}
          onClose={() => {
            make.reset();
            setMaking(false);
          }}
        />
      )}
    </>;
  return embedded ? <div className="flex min-w-0 flex-col gap-4"><div className="flex justify-end">{action}</div>{body}</div>
    : <Section id="channels" level={level} title={t('day100.names.toPeople')} lead={t('settings.channels.lead', { name: org.name })} action={action}>{body}</Section>;
}
