/** The destinations of a node: a form from the channel's fields, which kinds, the level, Test and the last result.
 *
 * Where what happens in an org, a project or a topic is sent for nobody in particular: a team's room
 * in a chat tool, a build system. Each destination is a webhook or any other kind the system has, made
 * from the fields that kind declares, with which kinds of event it carries (all, or the ones ticked),
 * whether it takes everything or only what is important, a Test whose answer is shown with what the far
 * side said when it fails, and how the last delivery went. One sentence says what is sent.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Pencil, Plus, Send, Trash2, Webhook } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption, offeredChoice } from '@/i18n/choices';

import { destinationsKey, destinationsQuery, createDestination, deleteDestination, notifiersQuery, testDestination, updateDestination } from '@/api/settings';
import { TestDialog } from '@/features/settings/org/Channels';
import { NotifierDialog, type NotifierFormValue } from '@/features/settings/NotifierForm';
import { takes } from '@/features/settings/refusals';
import type { Destination, Notifier, Org, Project, Topic } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { boxClasses, Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { PermissionGate } from '@/ui/PermissionGate';
import { RadioGroup } from '@/ui/RadioGroup';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { toast } from '@/ui/Toast';

/** The kinds of event a destination can be limited to, in the order a person looks for them. */
export const EVENT_KINDS = [
  'turn.completed',
  'turn.failed',
  'thread.blocked',
  'approval.requested',
  'milestone',
  'announcement',
  'provider.exhausted',
  'machine.offline',
  'environment.idle_stop',
  'environment.attention',
  'cap.warning',
  'cap.reached',
  'job.finished',
  'doc.mention',
  'doc.comment',
] as const;

type Level = 'all' | 'important';

/** The two things a destination has that a channel does not: which events it takes, and at what level. */
function Reach({
  kinds,
  limited,
  level,
  onKinds,
  onLimited,
  onLevel,
}: {
  kinds: string[];
  limited: boolean;
  level: Level;
  onKinds: (kinds: string[]) => void;
  onLimited: (limited: boolean) => void;
  onLevel: (level: Level) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-4">
      <RadioGroup
        label={t('settings.destinations.level')}
        value={level}
        onValueChange={onLevel}
        options={[
          { value: 'important', ...choiceOption(t, 'notificationLevel.important') },
          { value: 'all', ...choiceOption(t, 'notificationLevel.all') },
        ]}
      />
      <RadioGroup
        label={t('settings.destinations.which')}
        value={limited ? 'some' : 'every'}
        onValueChange={(next) => onLimited(next === 'some')}
        options={[
          { value: 'every', ...choiceOption(t, 'notificationKinds.every') },
          { value: 'some', ...choiceOption(t, 'notificationKinds.some') },
        ]}
      />
      {limited && (
        <div className="flex flex-col gap-2">
          <div role="group" aria-label={t('settings.destinations.kindsList')} className="grid grid-cols-1 gap-2 sm:grid-cols-2">
            {EVENT_KINDS.map((kind) => (
              <Checkbox
                key={kind}
                label={offeredChoice(t, 'eventKind', kind.replaceAll('.', '_'))?.name ?? t(`settings.destinations.kinds.${kind}`)}
                description={offeredChoice(t, 'eventKind', kind.replaceAll('.', '_'))?.description}
                checked={kinds.includes(kind)}
                onCheckedChange={(on) => onKinds(on ? [...kinds, kind] : kinds.filter((other) => other !== kind))}
              />
            ))}
          </div>
          {kinds.length === 0 && <EmptyState compact title={t('settings.destinations.tickOne')} />}
        </div>
      )}
    </div>
  );
}

function DestinationDialog({
  node,
  notifiers,
  existing,
  onClose,
}: {
  node: Org | Project | Topic;
  notifiers: readonly Notifier[];
  existing: Destination | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [kinds, setKinds] = useState<string[]>(existing?.kinds ?? []);
  // "Only the kinds I tick" is a choice of its own: while nothing is ticked yet the form waits for a tick instead of choosing for the person.
  const [limited, setLimited] = useState((existing?.kinds.length ?? 0) > 0);
  const [level, setLevel] = useState<Level>(existing?.level ?? 'important');
  // What is sent: no kinds at all means every kind.
  const carried = limited ? kinds : [];
  const save = useMutation({
    mutationFn: (value: NotifierFormValue) => {
      const secrets = Object.keys(value.secrets).length > 0 ? { secrets: value.secrets } : {};
      return existing === null
        ? createDestination(node.id, value.kind, { name: value.name, config: value.config, ...secrets, kinds: carried, level }, actKey({ ...value, kinds: carried, level }))
        : updateDestination(existing.id, { name: value.name, config: value.config, ...secrets, kinds: carried, level });
    },
    onSuccess: (destination) => {
      toast({ tone: 'success', title: existing === null ? t('settings.destinations.made', { name: destination.name }) : t('settings.destinations.saved', { name: destination.name }) });
      onClose();
    },
  });
  return (
    <NotifierDialog
      title={existing === null ? t('settings.destinations.addTitle', { name: node.name }) : t('settings.destinations.editTitle', { name: existing.name })}
      lead={t('settings.destinations.sends', { name: node.name })}
      notifiers={notifiers}
      {...(existing === null ? {} : { existing: { kind: existing.channel, name: existing.name, config: existing.config, secretsSet: existing.secrets_set } })}
      submitLabel={existing === null ? t('settings.destinations.make') : t('settings.save')}
      pending={save.isPending}
      error={save.error}
      extra={<Reach kinds={kinds} limited={limited} level={level} onKinds={setKinds} onLimited={setLimited} onLevel={setLevel} />}
      hold={limited && kinds.length === 0 ? t('settings.destinations.tickOne') : null}
      onSubmit={(value) => save.mutate(value)}
      onClose={onClose}
    />
  );
}

function DestinationRow({ destination, node, notifier, notifiers, may }: { destination: Destination; node: Org | Project | Topic; notifier: Notifier | undefined; notifiers: readonly Notifier[]; may: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [dialog, setDialog] = useState<'edit' | 'test' | 'remove' | null>(null);
  const done = () => void queryClient.invalidateQueries({ queryKey: destinationsKey(node.id) });
  const toggle = useMutation({ mutationFn: (enabled: boolean) => updateDestination(destination.id, { enabled }) });
  const remove = useMutation({
    mutationFn: () => deleteDestination(destination.id),
    onSuccess: () => {
      toast({ tone: 'success', title: t('settings.destinations.removed', { name: destination.name }) });
      setDialog(null);
    },
  });
  const enabled = toggle.isPending ? !destination.enabled : destination.enabled;
  return (
    <li className="flex flex-col gap-3 px-4 py-4" data-destination={destination.id}>
      <div className="flex flex-wrap items-start gap-x-4 gap-y-2">
        <Webhook className="mt-1 size-4 shrink-0 text-text-subtle" aria-hidden />
        <div className="min-w-0 flex-1 basis-[min(100%,14rem)]">
          <h3 className="text-base font-medium wrap-anywhere text-text">{destination.name}</h3>
          <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted">
            <span>{notifier?.title ?? t('notifications.kind.other')}</span><span>{destination.level === 'all' ? t('settings.destinations.levelAll') : t('settings.destinations.levelImportant')}</span>
            <span>{destination.kinds.length === 0 ? t('settings.destinations.everyKind') : t('settings.destinations.kindsCount', { count: destination.kinds.length })}</span>
          </p>
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
        {destination.last_ok_at !== null && (
          <p>
            {t('settings.channels.lastOk')} <RelativeTime at={destination.last_ok_at} />
          </p>
        )}
        {destination.last_ok_at === null && destination.last_error === null && <p>{t('settings.channels.neverSent')}</p>}
        {destination.last_error !== null && (
          <InlineAlert tone="danger" title={t('settings.channels.lastError')}>
            <span className="font-mono text-sm wrap-anywhere">{destination.last_error}</span>
          </InlineAlert>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <PermissionGate allowed={may} mode="block" reason={takes(t, 'settings.manage', node.name, node.owner)}>
          <Button size="sm" icon={<Send aria-hidden />} onClick={() => setDialog('test')} aria-label={t('settings.channels.testNamed', { name: destination.name })}>
            {t('settings.channels.test')}
          </Button>
        </PermissionGate>
        <PermissionGate allowed={may} mode="block" reason={takes(t, 'settings.manage', node.name, node.owner)}>
          <Button size="sm" icon={<Pencil aria-hidden />} onClick={() => setDialog('edit')} aria-label={t('settings.channels.editNamed', { name: destination.name })}>
            {t('settings.channels.edit')}
          </Button>
        </PermissionGate>
        {may && (
          <Button size="sm" variant="danger-ghost" icon={<Trash2 aria-hidden />} onClick={() => setDialog('remove')} aria-label={t('settings.channels.removeNamed', { name: destination.name })}>
            {t('settings.channels.remove')}
          </Button>
        )}
      </div>
      {dialog === 'test' && (
        <TestDialog title={t('settings.channels.testTitle', { name: destination.name })} needsAddress={false} run={() => testDestination(destination.id).catch((error: unknown) => { done(); throw error; })} onClose={() => setDialog(null)} />
      )}
      {dialog === 'edit' && <DestinationDialog node={node} notifiers={notifiers} existing={destination} onClose={() => setDialog(null)} />}
      <ConfirmDialog
        open={dialog === 'remove'}
        onOpenChange={(next) => !next && setDialog(null)}
        title={t('settings.destinations.removeTitle', { name: destination.name })}
        confirmLabel={t('settings.destinations.removeConfirm', { name: destination.name })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => remove.mutate()}
      >
        {t('settings.destinations.removeBody', { name: node.name })}
      </ConfirmDialog>
    </li>
  );
}

export function Destinations({ node, level = 2 }: { node: Org | Project | Topic; level?: 2 | 3 }) {
  const { t } = useTranslation();
  const may = holds(node.access, 'settings.manage');
  const notifiers = useQuery(notifiersQuery);
  const list = useQuery(destinationsQuery(node.id));
  const [making, setMaking] = useState(false);
  const kinds = (notifiers.data ?? []).filter((entry) => entry.targets.includes('destination'));
  return (
    <Section
      id="destinations"
      level={level}
      title={t('day100.names.toRoom')}
      lead={t('settings.destinations.lead', { name: node.name })}
      action={
        <PermissionGate allowed={may} mode="block" reason={takes(t, 'settings.manage', node.name, node.owner)}>
          <Button size="sm" variant="primary" icon={<Plus aria-hidden />} blocked={kinds.length === 0} onClick={() => setMaking(true)}>
            {t('settings.destinations.add')}
          </Button>
        </PermissionGate>
      }
    >
      <p className="text-sm text-text-muted">{t('settings.destinations.sends', { name: node.name })}</p>
      {list.isPending || notifiers.isPending ? (
        <SkeletonRows rows={2} />
      ) : list.isError ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : notifiers.isError ? (
        <ErrorState compact error={notifiers.error} retrying={notifiers.isRefetching} onRetry={() => void notifiers.refetch()} />
      ) : list.data.length === 0 ? (
        <EmptyState compact icon={<Webhook aria-hidden />} title={t('settings.destinations.none', { name: node.name })} description={may ? t('settings.destinations.noneAbout') : undefined} />
      ) : (
        <ul className={`${boxClasses} divide-y divide-border`} aria-label={t('settings.destinations.list', { name: node.name })}>
          {list.data.map((destination) => (
            <DestinationRow key={destination.id} destination={destination} node={node} notifier={kinds.find((entry) => entry.name === destination.channel)} notifiers={kinds} may={may} />
          ))}
        </ul>
      )}
      {making && <DestinationDialog node={node} notifiers={kinds} existing={null} onClose={() => setMaking(false)} />}
    </Section>
  );
}
