/** Shared ErrorState component (states); primitives are wrapped here.
 *
 * What a view shows in place of content it could not load, and what a form shows beside itself when
 * its request was refused: one explanation and one next action. Authorized recovery facts open
 * from that action; protocol codes and request identifiers are not interface copy.
 */
import { OctagonAlert, RefreshCw, WifiOff } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';

import { ApiError, invitationIssuerAuthorityLost } from '@/api/client';
import { SecondStepGate, waitsForSecondStep } from '@/features/auth/SecondStepGate';
import type { MissingAccess } from '@/generated/api-types';
import { permissionDenial } from '@/lib/permissions';
import { NetworkError } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { AvatarStack } from '@/ui/AvatarStack';
import { UserChip } from '@/ui/UserChip';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { useErrorText } from '@/ui/errorText';
import { PlanHostBlockers, planHostBlockers } from '@/ui/PlanHostBlockers';
import { StateLine } from '@/ui/StateLine';

export interface ErrorStateProps {
  error: unknown;
  /** A screen-specific subject, only where it adds to the explanation. */
  title?: ReactNode;
  titleAs?: 'h1' | 'h2' | 'h3' | 'p';
  /** A precise screen-specific explanation, while retaining the request's diagnostic details. */
  message?: ReactNode;
  /** Tries again when a lost connection or temporary refusal makes that useful. */
  onRetry?: (() => void) | undefined;
  /** Shown while a new try is under way. */
  retrying?: boolean;
  /** Less room round it, for a narrow bar. */
  compact?: boolean;
  /** The one next action; takes the place of a generic retry. */
  action?: ReactNode;
  /** A real recovery workflow, opened by the state's one action. */
  recovery?: { label: string; content: ReactNode };
  className?: string;
}

/** A view that could not show its content. */
export function ErrorState({
  error,
  title,
  titleAs,
  message,
  onRetry,
  retrying = false,
  compact = false,
  action,
  recovery,
  className,
}: ErrorStateProps) {
  // Verification is a workflow, not a generic retry or a second status paragraph.
  if (waitsForSecondStep(error)) return <SecondStepGate error={error} onDone={onRetry} className={className} />;
  return <FailureLine error={error} title={title} titleAs={titleAs} message={message} onRetry={onRetry} retrying={retrying} compact={compact} action={action} recovery={recovery} className={className} />;
}

function FailureLine({ error, title, titleAs, message, onRetry, retrying = false, compact = false, action, recovery, className }: ErrorStateProps) {
  const { t } = useTranslation();
  const [reviewing, setReviewing] = useState(false);
  const text = useErrorText()(error);
  const Icon = error instanceof NetworkError ? WifiOff : OctagonAlert;
  const missing = error instanceof ApiError && !invitationIssuerAuthorityLost(error) ? (error.extra('authz.denied')?.missing ?? []) : [];
  const faults = error instanceof ApiError ? error.faults() : [];
  const explanations = [...new Set(faults.map((fault) => fault.message))]
    .filter((message) => message !== text.message && message !== t('errors.request.invalid') && message !== t('errors.request.value_refused'));
  const hasFacts = missing.length > 0 || planHostBlockers(error).length > 0 || explanations.length > 0;
  const review = recovery ?? (hasFacts ? { label: missing.length > 0 ? t('ui.error.reviewAccess') : t('ui.error.review'), content: <>
    <MissingAccessList missing={missing} /><PlanHostBlockers error={error} />
    {explanations.length > 0 && <ul className="list-inside list-disc text-sm">{explanations.map((message) => <li key={message}>{message}</li>)}</ul>}
    {action}
  </> } : undefined);
  const next = review === undefined
    ? action ?? (onRetry !== undefined && text.retryable ? <Button size="sm" icon={<RefreshCw aria-hidden />} pending={retrying} onClick={onRetry}>{t('common.tryAgain')}</Button> : undefined)
    : <Button size="sm" onClick={() => setReviewing(true)}>{review.label}</Button>;
  return <>
    <StateLine role="alert" tone="danger" icon={<Icon aria-hidden />} title={title} titleAs={titleAs} message={message ?? text.message} action={next} compact={compact} className={className} />
    {review !== undefined && reviewing && <Dialog open onOpenChange={setReviewing} title={review.label}>
      <div className="flex flex-col gap-3">{review.content}</div>
    </Dialog>}
  </>;
}

/** What a refusal says is missing: the permissions, on what, and who can give them. */
export function MissingAccessList({ missing, className }: { missing: readonly MissingAccess[]; className?: string }) {
  const { t } = useTranslation();
  if (missing.length === 0) return null;
  return (
    <ul className={cx('flex flex-col gap-1 text-sm', className)}>
      {missing.map((entry, index) => {
        const place = entry.object.name ?? t('access.missing.thisItem');
        const what = entry.permissions.map((permission) => permissionDenial(t, permission, place)).join(' ');
        const who = entry.ask.map((person) => person.display_name).join(', ');
        const answer =
          entry.closed !== null
            ? t(`access.missing.closed.${entry.closed}`)
            : entry.ask.length === 0
              ? t('access.missing.nobody')
              : entry.ask_more > 0
                ? t('access.missing.askMore', { names: who, count: entry.ask_more })
                : t('access.missing.ask', { names: who });
        return (
          // Entries are a refusal's own list: given whole, never reordered.
          <li key={index} className="flex flex-col">
            <span>
              <span className="font-medium">{what}</span>
            </span>
            {entry.closed !== null || entry.ask.length === 0 ? <span className="text-text-muted">{answer}</span> : <span className="flex flex-wrap items-center gap-1 text-text-muted">
              <span className="sr-only">{answer}</span>
              <Trans i18nKey="access.missing.askPeople" components={{ people: entry.ask.length === 1 && entry.ask_more === 0 && entry.ask[0] !== undefined ? <UserChip user={entry.ask[0]} size={24} /> : <AvatarStack users={entry.ask} total={entry.ask.length + entry.ask_more} size={24} /> }} />
            </span>}
          </li>
        );
      })}
    </ul>
  );
}

export interface ErrorAlertProps {
  error: unknown;
  /** Other words for this error on this screen, where the screen knows more than the code says. */
  message?: ReactNode;
  action?: ReactNode;
  className?: string;
}

/** A request that was refused, said beside the form that made it. */
export function ErrorAlert({ error, message, action, className }: ErrorAlertProps) {
  return <FailureLine error={error} message={message} action={action} compact className={className} />;
}
