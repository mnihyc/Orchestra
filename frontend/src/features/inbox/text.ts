/** Notification action text in the reader's language, retaining authored words and legacy fallbacks. */
import type { TFunction } from 'i18next';

import { isPersonNotice } from '@/features/inbox/kinds';
import type { Notification } from '@/generated/api-types';
import type { TranslationKey } from '@/i18n';
import { foldName } from '@/lib/placeLabel';
import { formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { permissionLabel } from '@/lib/permissions';
import { errorText } from '@/ui/errorText';

/** This vocabulary is the one the server's first structured notification format writes. */
const ACTION_VERSION = 1;

const TITLES: Partial<Record<Notification['kind'], TranslationKey>> = {
  'turn.completed': 'inbox.actions.turnCompleted',
  'turn.failed': 'inbox.actions.turnFailed',
  'subagent.ended': 'inbox.actions.subagentEnded',
  'goal.ended': 'inbox.actions.goalEnded',
  'thread.blocked': 'inbox.actions.threadBlocked',
  'schedule.cannot_start': 'inbox.kinds.schedule_cannot_start',
  'approval.requested': 'inbox.actions.approvalRequested',
  'capability.requested': 'inbox.actions.capabilityRequested',
  'access.requested': 'inbox.actions.accessRequested',
  'access.decided': 'inbox.actions.accessDecided',
  mention: 'inbox.actions.mention',
  'chat.message': 'inbox.actions.chatMessage',
  'doc.mention': 'inbox.actions.docMention',
  'doc.comment': 'inbox.actions.docComment',
  'provider.exhausted': 'inbox.actions.providerExhausted',
  'provider.attention': 'inbox.actions.providerAttention',
  'machine.offline': 'inbox.actions.machineOffline',
  'environment.idle_stop': 'inbox.actions.environmentIdleStop',
  'environment.update': 'inbox.actions.environmentUpdate',
  'environment.attention': 'inbox.actions.environmentAttention',
  'cap.warning': 'inbox.actions.capWarning',
  'cap.reached': 'inbox.actions.capReached',
  'job.finished': 'inbox.actions.jobFinished',
  invitation: 'inbox.actions.invitation',
  'member.moved': 'inbox.actions.memberMoved',
  'sign_in.attention': 'inbox.actions.signInAttention',
  'notification.route_attention': 'inbox.actions.routeAttention',
};

function fields(value: unknown): Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

/** Actions use the same versioned, safe parameters as the row's text. */
export function notificationParameters(row: Notification): Record<string, unknown> {
  if (row.details !== null && row.details !== undefined) return row.details.v === ACTION_VERSION && row.details.action === row.kind ? fields(row.details.parameters) : {};
  return fields(row.data);
}

/** The exact text shown by an Inbox row; an unknown action version keeps the server's safe fallback. */
export function notificationText(row: Notification, t: TFunction, language: string, threadName?: string | null): { title: string; body: string; namesActor?: boolean } {
  const details = row.details;
  const structured = details?.v === ACTION_VERSION && details.action === row.kind;
  const data = fields(structured ? details.parameters : row.data);
  if (isPersonNotice(row.kind) && (structured || details === null || details === undefined)) {
    const action = t('inbox.actions.personNotify');
    const subject = typeof data.subject === 'string' ? data.subject : '';
    return { title: subject === '' ? action : t('inbox.actions.named', { name: subject, action }), body: row.body };
  }
  if ((structured || details === null || details === undefined) && row.kind === 'environment.update' && data.reason === 'requested') {
    const count = typeof data.affected_threads === 'number' && Number.isSafeInteger(data.affected_threads) && data.affected_threads >= 0 ? data.affected_threads : null;
    return {title:t('programVersions.noticeTitle'),body:t('programVersions.noticeThreads',{count:count ?? 0,number:count === null ? '–' : formatNumber(count,language)})};
  }
  if ((structured || details === null || details === undefined) && row.kind === 'environment.attention' && data.reason === 'temporary' && typeof data.expires_at === 'string') {
    const date = new Date(data.expires_at);
    if (Number.isFinite(date.getTime())) return {
      title: t('temporary.noticeTitle', { name: typeof data.subject === 'string' ? data.subject : row.title }),
      body: t('temporary.noticeBody', { when: new Intl.DateTimeFormat(language, { dateStyle: 'medium', timeStyle: 'short' }).format(date) }),
    };
  }
  if (row.kind === 'environment.attention' && typeof data.operation_kind === 'string') {
    const environmentCount = Array.isArray(data.environment_ids) ? data.environment_ids.length : 0;
    const threadCount = typeof data.thread_count === 'number' ? data.thread_count : Array.isArray(data.thread_ids) ? data.thread_ids.length : 0;
    const defaultCount = Array.isArray(data.default_topics) ? data.default_topics.length : 0;
    const providerCount = Array.isArray(data.provider_ids) ? data.provider_ids.length : 0;
    const resources = Array.isArray(data.resource_ids) ? data.resource_ids : [];
    const mountCount = resources.filter((row) => Array.isArray(row) && row[0] === 'mount').length;
    const serverCount = resources.filter((row) => Array.isArray(row) && row[0] === 'mcp_server').length;
    const name = typeof data.name === 'string' ? data.name : '';
    const state = data.state === 'failed' ? 'failed' : data.state === 'running' ? 'running' : 'done';
    const title = data.operation_kind === 'lost' ? t('loss.inboxTitle', { name }) : t(`operation.title.${data.operation_kind}.${state}`, { name, defaultValue: t('inbox.actions.environmentAttention') });
    return { title, body: [environmentCount > 0 ? t('loss.inboxBody', { count: environmentCount, number: formatNumber(environmentCount, language) }) : '', threadCount > 0 ? t('loss.threads', { count: threadCount, number: formatNumber(threadCount, language) }) : '', providerCount > 0 ? t('loss.providers', { count: providerCount, number: formatNumber(providerCount, language) }) : '', mountCount > 0 ? t('loss.mounts', { count: mountCount, number: formatNumber(mountCount, language) }) : '', serverCount > 0 ? t('loss.servers', { count: serverCount, number: formatNumber(serverCount, language) }) : '', defaultCount > 0 && data.default_lost === true ? t('loss.defaultTitle') : ''].filter(Boolean).join(' · ') };
  }
  if (row.kind === 'environment.attention' && data.loss === true) return { title: t('loss.inboxTitle', { name: typeof data.name === 'string' ? data.name : '' }), body: t('loss.inboxBody', { count: Array.isArray(data.environment_ids) ? data.environment_ids.length : 0, number: formatNumber(Array.isArray(data.environment_ids) ? data.environment_ids.length : 0, language) }) };
  if (row.kind === 'environment.attention' && data.default_lost === true) return { title: t('loss.defaultTitle'), body: t('loss.defaultBody') };
  const failedJob = row.kind === 'job.finished' && data.state === 'failed';
  const settled = (structured || details === null || details === undefined)
    && (row.kind === 'approval.requested' || row.kind === 'capability.requested' || row.kind === 'access.requested')
    && (data.approval_state === 'approved' || data.approval_state === 'answered' || data.approval_state === 'denied' || data.approval_state === 'cancelled' || data.approval_state === 'expired' || fields(row.data)._orch_approval_settled === true);
  const hasActor = row.actor !== null && row.actor !== undefined;
  const legacyOnly = threadName === undefined && !failedJob && !hasActor;
  if (!structured && !failedJob && !settled && (legacyOnly || details !== null && details !== undefined)) {
    return { title: row.title, body: row.body };
  }
  if (row.kind === 'milestone' || row.kind === 'announcement') return { title: row.title, body: row.body };
  const key = row.kind === 'job.finished' && data.state === 'failed' ? 'inbox.actions.jobFailed' : TITLES[row.kind];
  if (key === undefined) return { title: row.title, body: row.body };
  // A stored subject may be the thread's old name. Only its current reader-safe reference names it.
  const name = threadName !== undefined ? (typeof data.provider_name === 'string' ? data.provider_name : '')
    : typeof data.subject === 'string' ? data.subject : typeof data.provider_name === 'string' ? data.provider_name : '';
  let action = t(key);
  let namesActor = false;
  const actor = row.actor?.display_name;
  if (actor !== undefined) {
    switch (row.kind) {
      case 'access.requested': action = t('inbox.actions.accessRequestedBy', { name: actor }); namesActor = true; break;
      case 'access.decided': action = t(data.approved === true ? 'inbox.actions.accessApprovedBy' : data.approved === false ? 'inbox.actions.accessDeclinedBy' : 'inbox.actions.accessDecidedBy', { name: actor }); namesActor = true; break;
      case 'mention': action = t('inbox.actions.mentionedBy', { name: actor }); namesActor = true; break;
      case 'chat.message': action = t('inbox.actions.chatMessageBy', { name: actor }); namesActor = true; break;
      case 'doc.mention': action = t('inbox.actions.docMentionBy', { name: actor }); namesActor = true; break;
      case 'doc.comment': action = t('inbox.actions.docCommentBy', { name: actor }); namesActor = true; break;
    }
  }
  if (row.kind === 'goal.ended') action = t(data.how === 'met' ? 'inbox.actions.goalMet' : 'inbox.actions.goalStopped');
  if (row.kind === 'access.decided' && data.complete === true) { action = t('inbox.actions.accessComplete'); namesActor = false; }
  if (settled) {
    switch (data.approval_state) {
      case 'approved': action = actor === undefined ? t('inbox.actions.approvalAllowed') : t('inbox.actions.approvalAllowedBy', { name: actor }); break;
      case 'answered': action = actor === undefined ? t('inbox.actions.approvalAnswered') : t('inbox.actions.approvalAnsweredBy', { name: actor }); break;
      case 'denied': action = actor === undefined ? t('inbox.actions.approvalDenied') : t('inbox.actions.approvalDeniedBy', { name: actor }); break;
      case 'cancelled': action = t('inbox.actions.approvalCancelled'); break;
      case 'expired': action = t('inbox.actions.approvalExpired'); break;
      default: action = t('inbox.actions.approvalSettled'); break;
    }
    if(data.request_action==='proceed'||data.request_action==='keep_planning'||data.request_action==='run')action=t(`approvals.source.${data.request_action}`);
    // The action handles attribution, including its deliberate absence for expiry or withdrawal.
    namesActor = true;
  }
  const title = name === '' || (threadName !== null && threadName !== undefined && foldName(name) === foldName(threadName)) ? action : t('inbox.actions.named', { name, action });
  let body = '';
  switch (row.kind) {
    case 'thread.blocked':
      if (data.reason === 'repeat_limit') body=t('calls.repeatStopped', {
        kind:t(`calls.kinds.${typeof data.failure_kind==='string'?data.failure_kind:'any'}`, {defaultValue:t('calls.kinds.any')}),
        count:typeof data.count==='number'?data.count:0,
        occurrences:typeof data.count==='number'?formatNumber(data.count,language):'–',
        window:typeof data.window_secs==='number'?formatSpan(data.window_secs*1_000,{locale:language},'short'):'–',
      });
      break;
    case 'goal.ended':
    case 'turn.failed':
    case 'mention':
    case 'approval.requested':
    case 'chat.message':
    case 'doc.mention':
    case 'doc.comment':
    case 'invitation':
      body = row.body;
      break;
    case 'access.requested': {
      const permissions = Array.isArray(data.permissions) ? (data.permissions as unknown[]).filter((value): value is string => typeof value === 'string').map(permissionLabel) : [];
      body = permissions.length === 0 ? row.body : [t('inbox.actions.requestedPermissions', { permissions: new Intl.ListFormat(language).format(permissions) }), typeof data.reason === 'string' ? data.reason : ''].filter((part) => part !== '').join('\n');
      break;
    }
    case 'access.decided': {
      const permissions = Array.isArray(data.permissions) ? (data.permissions as unknown[]).filter((value): value is string => typeof value === 'string').map(permissionLabel) : [];
      body = data.approved === true && permissions.length > 0 ? t('inbox.actions.givenPermissions', { permissions: new Intl.ListFormat(language).format(permissions) }) : typeof data.note === 'string' ? data.note : '';
      break;
    }
    case 'job.finished':
      body = failedJob ? errorText({ code: typeof data.error_code === 'string' ? data.error_code : 'internal' },t,{locale:language}).message : '';
      break;
    case 'capability.requested':
      body = typeof data.capability === 'string' ? data.capability : '';
      break;
    case 'provider.attention':
    case 'provider.exhausted':
      body = t('inbox.actions.providerBody');
      break;
    case 'cap.warning':
    case 'cap.reached':
      body = t('inbox.actions.capBody');
      break;
    case 'notification.route_attention':
      body = t('inbox.actions.routeBody');
      break;
    case 'member.moved':
      body = t('inbox.actions.movedBody', { place: row.node?.name ?? t('inbox.actions.team') });
      break;
    case 'sign_in.attention': {
      const way = data.way === 'password' ? t('inbox.actions.password') : data.way === 'email_code'
        ? t('inbox.actions.emailCode') : data.way === 'totp' ? t('inbox.actions.totp') : null;
      const time = typeof data.until === 'string' ? new Date(data.until) : null;
      if (way !== null && time !== null && Number.isFinite(time.getTime())) {
        const until = new Intl.DateTimeFormat(language, { dateStyle: 'medium', timeStyle: 'short' }).format(time);
        body = t('inbox.actions.lockBody', { way, until });
      }
      break;
    }
  }
  return { title, body, ...(namesActor ? { namesActor: true } : {}) };
}
