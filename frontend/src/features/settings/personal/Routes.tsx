/** A person's routes: one card for each channel they can be reached through, with its switch, how long a notification stays unread first, how often a message goes, the kinds it carries, and for a bot the link that connects their chat.
 *
 * Nothing leaves the application for a person until they switch a way on, so every card begins off. A card says
 * what it is and whose ("Mail · this system", "Telegram · Acme's bot", "Ops room · a webhook of Acme"). Where a
 * way cannot reach the person yet it says what is missing and offers it: a confirmed address, a browser that
 * takes push, a bot that was started. Where it can, it says what it sends, how long a notification must stay
 * unread first and how often at most a message goes, and has Send a test. A way nobody set up has no card.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { FlaskConical, Mail, Pencil, Plus, Trash2, Unplug } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { StateLine } from '@/ui/StateLine';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';

import { meQuery, resendConfirmation } from '@/api/auth';
import { ApiError } from '@/api/client';
import { deleteChannel, deleteRoute, notificationChannelsKey, notificationChannelsQuery, notifiersQuery, setRoute, testChannel } from '@/api/notifications';
import { ALL_KINDS, KIND_LOOK } from '@/features/inbox/kinds';
import { ConnectDialog } from '@/features/settings/personal/Connect';
import { routesQuery } from '@/api/away';
import { DeviceReadiness } from '@/features/settings/personal/away/Device';
import { RetainedRoute, RouteOutcome } from '@/features/settings/personal/away/Outcome';
import { useAwayLive } from '@/features/settings/personal/away/live';
import { suppliedRoute } from '@/features/settings/personal/away/types';
import { useAwayAnchor } from '@/features/settings/personal/away/anchor';
import { WebhookDialog } from '@/features/settings/personal/Webhook';
import type { MeNotificationRouteSetBody, NotificationChannel, NotificationKind, Notifier } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { Popover } from '@/ui/Popover';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { TagChip } from '@/ui/TagChip';
import { toast } from '@/ui/Toast';

/** What a person is offered to fill in when they tick "only what I have not read after" and "not more often than": the examples of the page. */
export const DEFAULT_UNREAD_DELAY_MINUTES = 15;
export const DEFAULT_INTERVAL_MINUTES = 30;
/** A delay and an interval are each from one minute to a day, as the server holds them. */
export const LEAST_MINUTES = 1;
export const MOST_MINUTES = 1440;

/** The minutes a text says, or `null` where it says none that the server takes. */
export function minutesOf(text: string): number | null {
  if (!/^\d+$/.test(text.trim())) return null;
  const minutes = Number(text);
  return minutes >= LEAST_MINUTES && minutes <= MOST_MINUTES ? minutes : null;
}

/** What a channel is called at the head of its card: what it is, and whose. */
function useHeading(channel: NotificationChannel, notifier: Notifier | undefined): string {
  const { t } = useTranslation();
  const what = channel.kind === 'webhook' ? channel.name : kindName(channel.kind, notifier, t);
  const owner = channel.owner;
  let whose: string;
  if (owner === 'system') whose = channel.kind === 'webhook' ? t('notifications.whose.systemWebhook') : t('notifications.whose.system');
  else if (owner === 'me') whose = channel.kind === 'webhook' ? t('notifications.whose.mineWebhook') : t('notifications.whose.mine');
  else {
    const org = owner.org.name ?? '';
    whose =
      channel.kind === 'email'
        ? t('notifications.whose.orgEmail', { org })
        : channel.kind === 'telegram'
          ? t('notifications.whose.orgTelegram', { org })
          : channel.kind === 'webhook'
            ? t('notifications.whose.orgWebhook', { org })
            : t('notifications.whose.orgOther', { org });
  }
  return `${what} (${whose})`;
}

function kindName(kind: string, notifier: Notifier | undefined, t: TFunction): string {
  switch (kind) {
    case 'email':
      return t('notifications.kind.email');
    case 'telegram':
      return t('notifications.kind.telegram');
    case 'push':
      return t('notifications.kind.push');
    case 'webhook':
      return t('notifications.kind.webhook');
    default:
      return notifier?.title ?? t('notifications.kind.other');
  }
}

// ---------------------------------------------------------------- the kinds a route carries

function KindsChoice({ channel, save, busy }: { channel: NotificationChannel; save: (body: MeNotificationRouteSetBody) => void; busy: boolean }) {
  const { t } = useTranslation();
  const chosen = channel.route.kinds;
  const [only, setOnly] = useState(chosen.length > 0);
  const toggle = (kind: NotificationKind, on: boolean) => save({ kinds: on ? [...chosen, kind] : chosen.filter((each) => each !== kind) });
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span aria-hidden className="text-sm text-text-muted">
          {t('notifications.send.label')}
        </span>
        <SegmentedControl
          size="sm"
          label={t('notifications.send.label')}
          value={only ? 'only' : 'all'}
          onValueChange={(next) => {
            setOnly(next === 'only');
            if (next === 'all' && chosen.length > 0) save({ kinds: [] });
          }}
          segments={[
            { value: 'all', ...choiceOption(t, 'notificationDelivery.all'), disabled: busy },
            { value: 'only', ...choiceOption(t, 'notificationDelivery.only'), disabled: busy },
          ]}
        />
      </div>
      {only && (
        <div className="flex flex-wrap items-center gap-2">
          {chosen.map((kind) => (
            <TagChip key={kind} name={t(KIND_LOOK[kind].label)} color={0} onRemove={() => toggle(kind, false)} />
          ))}
          <Popover
            label={t('notifications.send.choose')}
            align="start"
            data-scroll-allow="choices" className="max-h-(--list-max-height) overflow-y-auto"
            trigger={
              <Button size="sm" icon={<Plus aria-hidden />}>
                {t('notifications.send.choose')}
              </Button>
            }
          >
            <div className="flex flex-col gap-1">
              {ALL_KINDS.map((kind) => (
                <Checkbox key={kind} label={t(KIND_LOOK[kind].label)} checked={chosen.includes(kind)} disabled={busy} onCheckedChange={(on) => toggle(kind, on)} />
              ))}
            </div>
          </Popover>
          {chosen.length === 0 && <span className="text-sm text-text-muted">{t('notifications.send.chooseOne')}</span>}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- a number of minutes, ticked and unticked

function Minutes({
  label,
  after,
  value,
  fallback,
  save,
  busy,
}: {
  /** The sentence before the number. */
  label: string;
  /** The words after it. */
  after: string;
  /** What the route holds: `null` for unticked. */
  value: number | null;
  fallback: number;
  save: (minutes: number | null) => void;
  busy: boolean;
}) {
  const { t } = useTranslation();
  const [text, setText] = useState(String(value ?? fallback));
  const [fault, setFault] = useState<string | null>(null);
  const on = value !== null;
  const commit = () => {
    const minutes = minutesOf(text);
    if (minutes === null) {
      setFault(t('notifications.minutesBetween', { least: LEAST_MINUTES, most: MOST_MINUTES }));
      return;
    }
    setFault(null);
    if (minutes !== value) save(minutes);
  };
  return (
    <div className="flex flex-col gap-1">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <Checkbox
          label={label}
          checked={on}
          disabled={busy}
          onCheckedChange={(next) => {
            setFault(null);
            if (next) {
              const minutes = minutesOf(text) ?? fallback;
              setText(String(minutes));
              save(minutes);
            } else {
              save(null);
            }
          }}
        />
        {/* The number and the words after it stay together: where the line is short they go below the sentence as one. */}
        <div className="flex min-w-0 items-center gap-2">
          <Input
            label={label}
            hideLabel
            type="number"
            inputMode="numeric"
            min={LEAST_MINUTES}
            max={MOST_MINUTES}
            value={text}
            disabled={!on || busy}
            fieldClassName="w-(--field-minutes)"
            onChange={(event) => setText(event.target.value)}
            onBlur={() => on && commit()}
            onKeyDown={(event) => {
              if (event.key === 'Enter') commit();
            }}
          />
          <span className="min-w-0 text-base text-text">{after}</span>
        </div>
      </div>
      {fault !== null && (
        <StateLine role="alert" tone="danger" compact message={fault} />
      )}
    </div>
  );
}

// ---------------------------------------------------------------- one card

type Done = { ok: boolean; error: string | null } | null;

function Details({ channel, save, busy }: { channel: NotificationChannel; save: (body: MeNotificationRouteSetBody) => void; busy: boolean }) {
  const { t } = useTranslation();
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<Done>(null);
  const [failure, setFailure] = useState<unknown>(null);
  const route = channel.route;
  const test = () => {
    setTesting(true);
    setResult(null);
    setFailure(null);
    testChannel(channel.id).then(
      (answer) => {
        setResult(answer);
        setTesting(false);
      },
      (error: unknown) => {
        setFailure(error);
        setTesting(false);
      },
    );
  };
  return (
    <div className="flex flex-col gap-3">
      <KindsChoice channel={channel} save={save} busy={busy} />
      <Minutes
        label={t('notifications.delay.label')}
        after={t('notifications.delay.after')}
        value={route.unread_delay_minutes}
        fallback={DEFAULT_UNREAD_DELAY_MINUTES}
        save={(minutes) => save({ unread_delay_minutes: minutes })}
        busy={busy}
      />
      <Minutes
        label={t('notifications.interval.label')}
        after={t('notifications.interval.after')}
        value={route.min_interval_minutes}
        fallback={DEFAULT_INTERVAL_MINUTES}
        save={(minutes) => save({ min_interval_minutes: minutes })}
        busy={busy}
      />
      {channel.kind !== 'push' && <div>
        <Button size="sm" icon={<FlaskConical aria-hidden />} pending={testing} onClick={test}>{t('away.outcome.test')}</Button>
      </div>}
      {result !== null && <InlineAlert tone={result.ok ? 'success' : 'danger'}>{result.ok ? t('away.outcome.accepted') : t('away.outcome.testFailed', { reason: result.error ?? '' })}</InlineAlert>}
      {failure !== null && <ErrorAlert error={failure} message={t('away.outcome.notConfirmed')} />}
      {/* A test that has just failed is the last failure too: said once. */}
      {channel.last_error !== null && channel.last_error !== result?.error && <InlineAlert tone="warning">{t('notifications.lastError', { error: channel.last_error })}</InlineAlert>}
    </div>
  );
}

/** What is missing before a way can reach the person, and the one thing that gives it. */
function Need({ channel, notifier }: { channel: NotificationChannel; notifier: Notifier | undefined }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const me = useQuery(meQuery).data;
  const [state, setState] = useState<{ pending: boolean; error: unknown; done: boolean }>({ pending: false, error: null, done: false });
  const [connecting, setConnecting] = useState(false);
  const need = channel.route.needs;
  const reload = () => queryClient.invalidateQueries({ queryKey: notificationChannelsKey });
  const run = (act: () => Promise<unknown>) => {
    setState({ pending: true, error: null, done: false });
    act().then(
      () => {
        setState({ pending: false, error: null, done: true });
      },
      (error: unknown) => setState({ pending: false, error, done: false }),
    );
  };
  if (need === null) return null;

  if (need === 'verified_email') {
    const address = me?.email ?? '';
    return (
      <InlineAlert tone="warning" title={t('notifications.need.verified_email', { email: address })}>
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" icon={<Mail aria-hidden />} pending={state.pending} onClick={() => run(() => resendConfirmation(me?.email ?? me?.username ?? ''))}>
            {t('notifications.need.resend')}
          </Button>
          {state.done && <span role="status">{t('notifications.need.sent', { email: address })}</span>}
        </div>
        {state.error !== null && <ErrorAlert error={state.error} />}
      </InlineAlert>
    );
  }

  if (need === 'device') return null;

  const name = kindName(channel.kind, notifier, t);
  return (
    <div className="flex flex-col gap-2">
      <p className="text-sm text-text-muted">{t('notifications.connect.notConnected')}</p>
      <div>
        <Button size="sm" icon={<Plus aria-hidden />} onClick={() => setConnecting(true)}>
          {t('notifications.connect.button', { name })}
        </Button>
      </div>
      <ConnectDialog channel={channel} name={name} open={connecting} onOpenChange={setConnecting} onLinked={() => void reload()} />
    </div>
  );
}

function ChannelCard({ channel, notifier }: { channel: NotificationChannel; notifier: Notifier | undefined }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const me = useQuery(meQuery).data;
  const route = channel.route;
  const outcome = suppliedRoute(route);
  const heading = useHeading(channel, notifier);
  const [asked, setAsked] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [editing, setEditing] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<unknown>(null);
  const [removePending, setRemovePending] = useState(false);

  const save = (body: MeNotificationRouteSetBody) => {
    setBusy(true);
    setError(null);
    return setRoute(channel.id, body).then(
      () => {
        setBusy(false);
        setAsked(null);
      },
      (failure: unknown) => {
        setBusy(false);
        setAsked(null);
        // Something is missing that the card did not know of: it is read again, and says what.
        if (failure instanceof ApiError && failure.is('route.not_ready')) void queryClient.invalidateQueries({ queryKey: notificationChannelsKey });
        else setError(failure);
      },
    );
  };

  const needs = route.needs !== null;
  const on = asked ?? route.enabled;
  const linked = channel.kind === 'telegram' && route.label !== null;
  const address = me?.email ?? null;
  const hint =
    channel.kind === 'email' && address !== null
      ? `${t('notifications.to', { address })} (${me?.email_verified === true ? t('notifications.confirmed') : t('notifications.notConfirmed')})`
      : linked
        ? t('notifications.connect.connectedAs', { label: route.label ?? '' })
        : undefined;

  return (
    <li>
      <section data-channel={channel.id} data-kind={channel.kind} aria-label={heading} className="flex flex-col gap-3 rounded-lg border border-border bg-surface px-4 py-3">
        <Switch
          label={<span className="font-medium">{heading}</span>}
          hint={hint}
          checked={on}
          disabled={busy && asked !== null}
          onCheckedChange={(next) => {
            // Switching on a way that cannot reach the person yet is not refused in silence: the card says what is missing, just below.
            if (next && needs) return;
            setAsked(next);
            void save({ enabled: next });
          }}
        />
        {needs && <Need channel={channel} notifier={notifier} />}
        {channel.kind === 'push' && <DeviceReadiness channelId={channel.id} />}
        {outcome !== null && <RouteOutcome route={outcome} />}
        {!needs && <Details channel={channel} save={(body) => void save(body)} busy={busy} />}
        {!needs && linked && (
          <div>
            <Button
              size="sm"
              variant="ghost"
              icon={<Unplug aria-hidden />}
              onClick={() => {
                deleteRoute(channel.id).catch((failure: unknown) => setError(failure));
              }}
            >
              {t('notifications.connect.disconnect')}
            </Button>
          </div>
        )}
        {channel.can_edit && channel.kind === 'webhook' && (
          <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
            <Button size="sm" icon={<Pencil aria-hidden />} onClick={() => setEditing(true)}>
              {t('notifications.webhook.edit')}
            </Button>
            <Button size="sm" icon={<Trash2 aria-hidden />} variant="danger-ghost" onClick={() => setRemoving(true)}>
              {t('notifications.webhook.remove')}
            </Button>
          </div>
        )}
        {error !== null && <ErrorAlert error={error} />}
      </section>
      {editing && <WebhookDialog channel={channel} open={editing} onOpenChange={setEditing} />}
      <ConfirmDialog
        open={removing}
        onOpenChange={setRemoving}
        title={t('notifications.webhook.removeTitle', { name: channel.name })}
        confirmLabel={t('notifications.webhook.removeConfirm', { name: channel.name })}
        danger
        pending={removePending}
        error={removeError}
        onConfirm={() => {
          setRemovePending(true);
          setRemoveError(null);
          deleteChannel(channel.id).then(
            () => {
              setRemovePending(false);
              setRemoving(false);
              toast({ tone: 'success', title: t('notifications.webhook.removed', { name: channel.name }) });
            },
            (failure: unknown) => {
              setRemovePending(false);
              setRemoveError(failure);
            },
          );
        }}
      >
        {t('notifications.webhook.removeWhat')}
      </ConfirmDialog>
    </li>
  );
}

// ---------------------------------------------------------------- all of them

/** The ways out of the application that can reach the person, one card each, and the way to add a webhook of their own. */
export function OutsideOrch() {
  const { t } = useTranslation();
  useAwayLive();
  const channels = useQuery(notificationChannelsQuery);
  const routes = useQuery(routesQuery);
  useAwayAnchor(channels.dataUpdatedAt + routes.dataUpdatedAt);
  const notifiers = useQuery(notifiersQuery);
  const [adding, setAdding] = useState(false);
  const byName = new Map((notifiers.data ?? []).map((notifier) => [notifier.name, notifier]));
  const current = new Map((routes.data ?? []).filter((route) => route.channel_id !== null && route.health !== 'channel_removed').map((route) => [route.channel_id, route]));
  const list = channels.data?.map((channel) => {
    const route = current.get(channel.id);
    return { ...channel, route: route === undefined ? channel.route : { ...route, channel_id: channel.id } };
  });
  const retained = (routes.data ?? []).filter((route) => route.health === 'channel_removed');
  return (
    <div className="flex flex-col gap-3">
      {list === undefined && channels.isPending ? (
        <SkeletonGroup className="flex flex-col gap-3">
          {[0, 1].map((index) => (
            <div key={index} className="flex flex-col gap-2 rounded-lg border border-border bg-surface px-4 py-3">
              <Skeleton className="h-5 w-1/3" />
              <Skeleton className="h-4 w-1/2" />
            </div>
          ))}
        </SkeletonGroup>
      ) : list === undefined ? (
        <ErrorState compact error={channels.error} retrying={channels.isRefetching} onRetry={() => void channels.refetch()} />
      ) : list.length === 0 ? (
        <EmptyState compact title={t('notifications.outside.none')} />
      ) : (
        <ul className="flex flex-col gap-3">
          {list.map((channel) => (
            <ChannelCard key={channel.id} channel={channel} notifier={byName.get(channel.kind)} />
          ))}
        </ul>
      )}
      {retained.length > 0 && <ul className="flex flex-col gap-3">{retained.map((route) => <RetainedRoute key={route.id} route={route} />)}</ul>}
      {routes.isError && <ErrorState compact error={routes.error} retrying={routes.isRefetching} onRetry={() => void routes.refetch()} />}
      {channels.isError && list !== undefined && <ErrorState compact error={channels.error} retrying={channels.isRefetching} onRetry={() => void channels.refetch()} />}
      <div>
        <Button icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>
          {t('notifications.webhook.add')}
        </Button>
      </div>
      {adding && <WebhookDialog channel={null} open={adding} onOpenChange={setAdding} />}
    </div>
  );
}
