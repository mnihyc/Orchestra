/** Context, read state and target link.
 *
 * One row of the Inbox: its thread's current readable name, one event and its place, useful authored detail,
 * when, and whether it is read. Personal/resource notices retain their own title. The row is a
 * real link to what it is about: a click opens it and marks it read, `Mod+click` and a middle click open a tab
 * of the browser and mark it all the same. Its menu, which a right click opens as well, marks read one row or all
 * that came from one place.
 */
import { ThreadStateIcon } from '@/ui/ThreadState';
import { useNavigate } from '@tanstack/react-router';
import { CheckCheck, Ellipsis, ExternalLink, History, Link as LinkIcon, SquareArrowOutUpRight, TriangleAlert } from 'lucide-react';
import { type MouseEvent, useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';

import { openChange } from '@/features/change/target';
import { lookOf, TONE_DISC } from '@/features/inbox/kinds';
import { placeOf } from '@/features/inbox/model';
import { markNotificationsRead } from '@/features/inbox/read';
import { DecisionStatus, type DecisionLine } from '@/features/approvals/Card';
import { notificationParameters, notificationText } from '@/features/inbox/text';
import { KeepEnvironmentButton } from '@/features/resources/environments/Temporary';
import { EnvironmentExportButton, SnapshotDownloadButton } from '@/features/resources/environments/WorkspaceExport';
import { notificationPlace, notificationThread } from '@/features/inbox/presentation';
import type { Notification, UserRef } from '@/generated/api-types';
import { foldName } from '@/lib/placeLabel';
import { copyText } from '@/ui/clipboard';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import { catchupUrl } from './away/model';
import { RouteNotice } from './away/Notice';
import { Button } from '@/ui/Button';
import { UserChip } from '@/ui/UserChip';
import { CallHelp } from '@/features/calls/Help';
import { NotificationFiles } from '@/features/files/MessageFiles';

const ACTOR_MARKS = ['\u0001', '\u0002'] as const;

/** Two marked renderings locate the actor slot without mistaking a resource's matching name for a person. */
function ActorText({ text, actor, templates }: { text: string; actor: UserRef | null | undefined; templates: readonly [string, string] | undefined }) {
  if (actor === null || actor === undefined || templates === undefined) return text;
  const [one, two] = templates;
  let at = 0;
  while (at < one.length && one[at] === two[at]) at += 1;
  if (one[at] !== ACTOR_MARKS[0] || two[at] !== ACTOR_MARKS[1]) return text;
  return <>{one.slice(0, at)}<UserChip user={actor} size={24} className="pointer-events-auto relative z-10" />{one.slice(at + 1)}</>;
}

export function NotificationRow({ row, byPlace = false, labels }: { row: Notification; byPlace?: boolean; labels?: ReadonlyMap<string, string> }) {
  const { t, i18n } = useTranslation();
  const [kept, setKept] = useState(false);
  const parameters = notificationParameters(row);
  const lostEnvironments = row.kind === 'environment.attention' && (parameters.loss === true || parameters.operation_kind === 'recover' && parameters.state === 'failed') && Array.isArray(parameters.environment_ids) ? parameters.environment_ids.filter((id): id is string => typeof id === 'string') : [];
  const temporary = row.kind === 'environment.attention' && parameters.reason === 'temporary' && typeof parameters.environment_id === 'string' ? parameters.environment_id : null;
  const exported = row.kind === 'job.finished' && parameters.workspace_export === true && typeof parameters.snapshot_id === 'string' ? parameters.snapshot_id : null;
  const exportName = typeof parameters.subject === 'string' ? parameters.subject : row.title;
  const thread = notificationThread(row);
  const hasThread = row.thread_id !== null || thread !== null;
  const readableThreadName = thread !== null && !thread.redacted ? thread.name?.trim() ?? null : null;
  const threadName = readableThreadName === '' ? null : readableThreadName;
  const settled=(row.kind==='approval.requested'||row.kind==='capability.requested'||row.kind==='access.requested')&&typeof parameters.approval_state==='string'&&['approved','answered','denied','cancelled','expired'].includes(parameters.approval_state);
  const text = notificationText(settled?{...row,actor:null}:row, t, i18n.resolvedLanguage ?? i18n.language, hasThread ? threadName : undefined);
  const actor = row.actor;
  const actorTemplates: readonly [string, string] | undefined = !settled && text.namesActor === true && actor !== null && actor !== undefined
    ? [notificationText({ ...row, actor: { ...actor, display_name: ACTOR_MARKS[0] } }, t, i18n.resolvedLanguage ?? i18n.language, hasThread ? threadName : undefined).title,
      notificationText({ ...row, actor: { ...actor, display_name: ACTOR_MARKS[1] } }, t, i18n.resolvedLanguage ?? i18n.language, hasThread ? threadName : undefined).title] : undefined;
  const decision=typeof parameters.request_decision==='object'&&parameters.request_decision!==null&&!Array.isArray(parameters.request_decision)?parameters.request_decision as Record<string,unknown>:{};
  const given=row.kind==='access.requested'&&parameters.approval_state==='approved';
  const outcome:DecisionLine|null=settled?{word:given?t('approvals.quick.given'):text.title,by:row.actor??null,at:typeof parameters.decided_at==='string'?parameters.decided_at:null,given,expiresAt:typeof decision.expires_at==='string'?decision.expires_at:null,note:parameters.approval_state==='denied'&&typeof decision.note==='string'?decision.note:null}:null;
  const where = notificationPlace(row);
  const namedPlace = where === null ? null : labels?.get(where.id) ?? where.name;
  const generic = threadName !== null && [t('chat.kind.thread'), i18n.getFixedT('en')('chat.kind.thread')].some((name) => foldName(name) === foldName(threadName));
  const shownThread = generic ? namedPlace === null ? t('inbox.row.thread') : t('inbox.row.threadInPlace', { thread: threadName, place: namedPlace }) : threadName;
  const title = kept && temporary !== null ? exportName : hasThread ? shownThread ?? t(thread?.redacted === true ? 'inbox.row.privateThread' : 'inbox.row.thread')
    : text.title === '' ? t('inbox.row.other') : text.title;
  const event = !settled && hasThread && text.title !== '' && foldName(text.title) !== foldName(title) ? text.title : null;
  const navigate = useNavigate();
  const look = lookOf(row.kind);
  const attention = row.kind === 'notification.route_attention';
  const Icon = attention ? TriangleAlert : look.icon;
  const unread = row.read_at === null;
  const context = !generic && namedPlace !== null && foldName(namedPlace) !== foldName(title) && !(byPlace && row.group?.place?.id === where?.id) ? namedPlace : null;
  const body = text.body !== '' && foldName(text.body) !== foldName(title) && (event === null || foldName(text.body) !== foldName(event)) ? text.body : null;
  const catchup = byPlace ? catchupUrl(row) : null;
  const place = placeOf(row);
  const chat = row.chat_id;
  const placeName = place === thread?.id ? title : context ?? (row.node?.redacted === false ? row.node.name : null) ?? title;

  /** Goes to what the row is about, and marks the row read: the person has seen it. */
  const open = () => {
    void markNotificationsRead({ ids: [row.id] });
    void navigate({ href: row.url });
  };

  const onClick = (event: MouseEvent<HTMLAnchorElement>) => {
    // A modified click is the browser's: it opens a tab, and the row is read all the same.
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      void markNotificationsRead({ ids: [row.id] });
      return;
    }
    event.preventDefault();
    open();
  };

  const copyLink = () => copyText(new URL(row.url, window.location.origin).toString(), t('common.linkCopied'));

  const entries: MenuEntry[] = [
    { id: 'open', label: t('inbox.row.open'), icon: <ExternalLink aria-hidden />, onSelect: open },
    {
      id: 'tab',
      label: t('inbox.row.openNewTab'),
      icon: <SquareArrowOutUpRight aria-hidden />,
      onSelect: () => {
        void markNotificationsRead({ ids: [row.id] });
        window.open(row.url, '_blank', 'noopener');
      },
    },
    { type: 'separator', id: 'separator-read' },
    {
      id: 'read',
      label: t('inbox.row.markRead'),
      icon: <CheckCheck aria-hidden />,
      disabled: unread ? false : t('inbox.row.alreadyRead'),
      onSelect: () => void markNotificationsRead({ ids: [row.id] }),
    },
    ...(chat !== null
      ? [
          {
            id: 'chat-read',
            label: t('inbox.row.markChatRead'),
            icon: <CheckCheck aria-hidden />,
            onSelect: () => void markNotificationsRead({ chat }),
          } satisfies MenuEntry,
        ]
      : place !== null
        ? [
            {
              id: 'place-read',
              label: t('inbox.row.markPlaceRead', { place: placeName }),
              icon: <CheckCheck aria-hidden />,
              onSelect: () => void markNotificationsRead({ place }),
            } satisfies MenuEntry,
          ]
        : []),
    { type: 'separator', id: 'separator-link' },
    { id: 'copy', label: t('inbox.row.copyLink'), icon: <LinkIcon aria-hidden />, onSelect: () => void copyLink() },
  ];

  return (
    <ContextMenu label={t('inbox.row.menu')} entries={entries}>
      <li data-unread={unread} data-kind={row.kind} className="group relative flex items-start gap-1 px-3 py-3 text-base leading-6 transition-colors hover:bg-bg-subtle">
        <div className="flex min-w-0 flex-1 flex-col gap-2"><div className="relative flex min-w-0 flex-1 items-start gap-3 rounded-md"><a
          href={row.url}
          onClick={onClick}
          onAuxClick={(event) => event.button === 1 && void markNotificationsRead({ ids: [row.id] })}
          aria-label={title}
          className="focus-inset absolute inset-0 rounded-md"
        />
          <span className="pointer-events-none flex h-6 shrink-0 items-center"><span className={cx('flex size-8 shrink-0 items-center justify-center rounded-full [&_svg]:size-4', TONE_DISC[attention ? 'warning' : look.tone])}>
            {hasThread ? <ThreadStateIcon display={thread?.redacted === false ? thread.thread_display : undefined} /> : <Icon aria-hidden />}
          </span></span>
          <span className="pointer-events-none flex min-w-0 flex-1 flex-col gap-1">
            <span className="flex items-baseline gap-2">
              <span data-inbox-title className={cx('min-w-0 flex-1 text-base leading-6 wrap-anywhere', unread ? 'font-semibold text-text' : 'font-normal text-text-muted')}>
                {unread && <span className="sr-only">{t('inbox.row.unread')}: </span>}
                {!hasThread && !settled && text.namesActor === true ? <ActorText text={title} actor={row.actor} templates={actorTemplates} /> : title}
              </span>
              <RelativeTime at={row.created_at} className="pointer-events-auto relative z-10 shrink-0 text-xs text-text-subtle" />
            </span>
            {(event !== null || context !== null) && <span className="flex flex-wrap gap-x-1 text-sm text-text-muted wrap-anywhere">
              {event !== null && <span data-inbox-event>{text.namesActor === true ? <ActorText text={event} actor={row.actor} templates={actorTemplates} /> : event}</span>}
              {context !== null && <span data-inbox-place className="text-text-subtle">{t('inbox.row.inPlace', { place: context })}</span>}
            </span>}
            {!settled && row.actor !== null && row.actor !== undefined && text.namesActor !== true && <span data-inbox-person className="flex items-center gap-1 text-sm text-text-muted"><Trans i18nKey="inbox.row.byPersonChip" components={{ person: <UserChip user={row.actor} size={24} className="pointer-events-auto relative z-10" /> }} /></span>}
            {kept ? <span className="text-sm text-text-muted">{t('temporary.kept')}</span> : attention ? <span className="pointer-events-auto relative z-10"><RouteNotice row={row} /></span> : body !== null && <span className={cx('text-sm wrap-anywhere', unread ? 'text-text' : 'text-text-muted')}>{body}</span>}
            {row.waiting_for_me && <span className="text-xs font-medium text-warning-text">{t('away.inbox.waiting')}</span>}
          </span>
        </div>
        {outcome!==null&&<div className="ps-[calc(var(--spacing-4)*2+var(--spacing-3))]"><DecisionStatus line={outcome}/></div>}
        <NotificationFiles row={row} />
        {parameters.operation_kind === 'recover' && parameters.state === 'running' && typeof parameters.operation_id === 'string' && Array.isArray(parameters.environment_ids) && <div className="flex gap-2 ps-[calc(var(--spacing-4)*2+var(--spacing-3))]"><Button size="sm" onClick={() => openChange({ kind: 'environments', ids: (parameters.environment_ids as unknown[]).filter((id): id is string => typeof id === 'string') }, { kind: 'progress', batch: parameters.operation_id as string })}>{t('operation.showProgress')}</Button></div>}
        {lostEnvironments.length > 0 && <div className="flex flex-wrap gap-2 ps-[calc(var(--spacing-4)*2+var(--spacing-3))]"><Button size="sm" onClick={() => openChange({ kind: 'environments', ids: lostEnvironments }, { kind: 'recover' })}>{t('loss.recover')}</Button></div>}
        {(temporary !== null && !kept || exported !== null) && <div className="flex flex-wrap gap-2 ps-[calc(var(--spacing-4)*2+var(--spacing-3))]">
          {temporary !== null && !kept && <><KeepEnvironmentButton id={temporary} onKept={() => setKept(true)} /><EnvironmentExportButton id={temporary} name={exportName} /></>}
          {exported !== null && <SnapshotDownloadButton id={exported} name={exportName} />}
        </div>}</div>
        {catchup !== null && <Button size="sm" variant="ghost" asChild><a href={catchup}><History aria-hidden />{t('away.inbox.catchup')}</a></Button>}
        {row.kind === 'thread.blocked' && parameters.reason === 'repeat_limit' && <CallHelp section="guard" />}
        <span className="flex h-6 w-4 shrink-0 items-center justify-center" aria-hidden>
          {unread && <span data-testid="unread-dot" className="size-2 rounded-full bg-accent" />}
        </span>
        <span className="flex h-6 shrink-0 items-center" data-inbox-menu><Menu
          label={t('inbox.row.menu')}
          align="end"
          entries={entries}
          trigger={<IconButton size="sm" label={t('inbox.row.menu')} icon={<Ellipsis aria-hidden />} />}
        /></span>
      </li>
    </ContextMenu>
  );
}
