/** What each kind of notification looks like in the Inbox: its icon, the tone of the icon and its name.
 *
 * The server writes the title and the first line of each row; this says what kind of thing the row
 * is, in the reader's language, so that a list of mixed rows can be read at a glance. A kind this
 * build does not know (a newer server) is drawn plain, and named by its key.
 */
import {
  ArrowRightLeft,
  AtSign,
  Ban,
  Bell,
  BellOff,
  Bot,
  CircleCheck,
  CircleX,
  Flag,
  Gauge,
  KeyRound,
  ListChecks,
  type LucideIcon,
  Mail,
  Megaphone,
  MessageCircle,
  MessageSquareText,
  Moon,
  OctagonAlert,
  PlugZap,
  RefreshCw,
  ServerOff,
  ShieldAlert,
  ShieldQuestionMark,
  Target,
  TriangleAlert,
  UserCheck,
  UserPlus,
} from 'lucide-react';

import type { NotificationKind } from '@/generated/api-types';

export type KindTone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger';

export interface KindLook {
  icon: LucideIcon;
  tone: KindTone;
  /** The catalogue key of its name. */
  label:
    | 'inbox.kinds.turn_completed'
    | 'inbox.kinds.turn_failed'
    | 'inbox.kinds.subagent_ended'
    | 'inbox.kinds.goal_ended'
    | 'inbox.kinds.thread_blocked'
    | 'inbox.kinds.approval_requested'
    | 'inbox.kinds.capability_requested'
    | 'inbox.kinds.access_requested'
    | 'inbox.kinds.access_decided'
    | 'inbox.kinds.milestone'
    | 'inbox.kinds.announcement'
    | 'inbox.kinds.mention'
    | 'inbox.kinds.chat_message'
    | 'inbox.kinds.doc_mention'
    | 'inbox.kinds.person_notify'
    | 'inbox.kinds.doc_comment'
    | 'inbox.kinds.provider_exhausted'
    | 'inbox.kinds.provider_attention'
    | 'inbox.kinds.machine_offline'
    | 'inbox.kinds.environment_idle_stop'
    | 'inbox.kinds.environment_update'
    | 'inbox.kinds.environment_attention'
    | 'inbox.kinds.cap_warning'
    | 'inbox.kinds.cap_reached'
    | 'inbox.kinds.job_finished'
    | 'inbox.kinds.schedule_cannot_start'
    | 'inbox.kinds.invitation'
    | 'inbox.kinds.member_moved'
    | 'inbox.kinds.sign_in_attention'
    | 'inbox.kinds.route_attention';
}

/** Every kind the contract names, in the order a list of them reads best: what needs the person first. */
export const KIND_LOOK = {
  'approval.requested': { icon: ShieldQuestionMark, tone: 'warning', label: 'inbox.kinds.approval_requested' },
  'capability.requested': { icon: KeyRound, tone: 'warning', label: 'inbox.kinds.capability_requested' },
  'access.requested': { icon: UserPlus, tone: 'warning', label: 'inbox.kinds.access_requested' },
  'access.decided': { icon: UserCheck, tone: 'accent', label: 'inbox.kinds.access_decided' },
  mention: { icon: AtSign, tone: 'accent', label: 'inbox.kinds.mention' },
  'doc.mention': { icon: AtSign, tone: 'accent', label: 'inbox.kinds.doc_mention' },
  'person.notify': { icon: Bell, tone: 'accent', label: 'inbox.kinds.person_notify' },
  'doc.comment': { icon: MessageSquareText, tone: 'neutral', label: 'inbox.kinds.doc_comment' },
  'chat.message': { icon: MessageCircle, tone: 'neutral', label: 'inbox.kinds.chat_message' },
  'turn.completed': { icon: CircleCheck, tone: 'success', label: 'inbox.kinds.turn_completed' },
  'turn.failed': { icon: CircleX, tone: 'danger', label: 'inbox.kinds.turn_failed' },
  'subagent.ended': { icon: Bot, tone: 'neutral', label: 'inbox.kinds.subagent_ended' },
  'goal.ended': { icon: Target, tone: 'accent', label: 'inbox.kinds.goal_ended' },
  'thread.blocked': { icon: OctagonAlert, tone: 'warning', label: 'inbox.kinds.thread_blocked' },
  milestone: { icon: Flag, tone: 'accent', label: 'inbox.kinds.milestone' },
  announcement: { icon: Megaphone, tone: 'accent', label: 'inbox.kinds.announcement' },
  'provider.exhausted': { icon: Gauge, tone: 'danger', label: 'inbox.kinds.provider_exhausted' },
  'provider.attention': { icon: PlugZap, tone: 'warning', label: 'inbox.kinds.provider_attention' },
  'machine.offline': { icon: ServerOff, tone: 'warning', label: 'inbox.kinds.machine_offline' },
  'environment.idle_stop': { icon: Moon, tone: 'neutral', label: 'inbox.kinds.environment_idle_stop' },
  'environment.update': { icon: RefreshCw, tone: 'neutral', label: 'inbox.kinds.environment_update' },
  'environment.attention': { icon: TriangleAlert, tone: 'warning', label: 'inbox.kinds.environment_attention' },
  'cap.warning': { icon: Gauge, tone: 'warning', label: 'inbox.kinds.cap_warning' },
  'cap.reached': { icon: Ban, tone: 'danger', label: 'inbox.kinds.cap_reached' },
  'job.finished': { icon: ListChecks, tone: 'neutral', label: 'inbox.kinds.job_finished' },
  'schedule.cannot_start': { icon: OctagonAlert, tone: 'warning', label: 'inbox.kinds.schedule_cannot_start' },
  invitation: { icon: Mail, tone: 'accent', label: 'inbox.kinds.invitation' },
  'member.moved': { icon: ArrowRightLeft, tone: 'neutral', label: 'inbox.kinds.member_moved' },
  'sign_in.attention': { icon: ShieldAlert, tone: 'warning', label: 'inbox.kinds.sign_in_attention' },
  'notification.route_attention': { icon: BellOff, tone: 'warning', label: 'inbox.kinds.route_attention' },
} satisfies Record<NotificationKind, KindLook> & Record<'person.notify', KindLook>;

/** The kinds in that order. */
export const ALL_KINDS = Object.keys(KIND_LOOK) as NotificationKind[];

/** What the Mentions filter holds: somebody named the person where they can read it. */
export const MENTION_KINDS: readonly string[] = ['mention', 'doc.mention', 'person.notify'];

/** Whether a notice was explicitly sent by a person. */
export function isPersonNotice(kind: string): boolean { return kind === 'person.notify'; }

/** How a kind is drawn; a kind this build does not know gets a plain bell. */
export function lookOf(kind: string): { icon: LucideIcon; tone: KindTone; label: KindLook['label'] | null } {
  const known = (KIND_LOOK as Partial<Record<string, KindLook>>)[kind];
  return known ?? { icon: Bell, tone: 'neutral', label: null };
}

/** The classes of the disc an icon stands in, by tone. */
export const TONE_DISC: Record<KindTone, string> = {
  neutral: 'bg-bg-muted text-text-muted',
  accent: 'bg-accent-subtle text-accent-text',
  success: 'bg-success-subtle text-success-text',
  warning: 'bg-warning-subtle text-warning',
  danger: 'bg-danger-subtle text-danger-text',
};
