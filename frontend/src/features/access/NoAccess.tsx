/** A real refusal names what is missing and offers the ordinary access request when it is served.
 *
 * What stands in place of a screen whose content the server refused: the kind of thing, "You don't have
 * access", what is missing and whom to ask, and the way on. For most people that is the request: the three
 * choices for that thing, with the least that covers what was missing selected, a line for a reason, Send.
 * Administrators already reach what they administer; a refusal never offers a self-grant step.
 */
import { useRouter } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { Button } from '@/ui/Button';
import { ErrorState, MissingAccessList } from '@/ui/ErrorState';

import { AccessRequest } from './AccessRequest';

export interface NoAccessProps {
  error: ApiError;
  /** Asks the screen again, which the person does once they hold what was missing. */
  onRetry?: (() => void) | undefined;
  className?: string | undefined;
  /** The thread the person was trying to act on, so that each approver sees why they are asked. */
  forThread?: string | undefined;
}

/** The kind of a thing, in the reader's words. */
function useKindName(kind: string): string {
  const { t } = useTranslation();
  // A future kind still gets a human label, never its protocol identity.
  const known = ['org', 'project', 'topic', 'thread', 'document', 'chat', 'provider', 'agent', 'environment', 'host', 'machine', 'template', 'mcp_server', 'mount', 'skill'];
  return known.includes(kind) ? t(`access.kinds.${kind}` as 'access.kinds.org') : t('everything.unknownKind');
}

export function NoAccess({ error, onRetry, className, forThread }: NoAccessProps) {
  const { t } = useTranslation();
  const router = useRouter();
  const denied = error.extra('authz.denied');
  const missing = denied?.missing ?? [];
  const object = error.problem.object as { kind?: string; id?: string } | undefined;
  const objectId = typeof object?.id === 'string' ? object.id : (missing[0]?.object.id ?? '');
  const kind = typeof object?.kind === 'string' ? object.kind : (missing[0]?.object.kind ?? 'org');
  const main = missing.find((entry) => entry.object.id === objectId) ?? missing[0];
  const name = main?.object.name ?? null;
  const kindName = useKindName(kind);

  const retry = () => {
    if (onRetry !== undefined) onRetry();
    else void router.invalidate();
  };

  const requestable = denied?.requestable === true && missing.some((entry) => entry.closed === null);

  const way = requestable ? <AccessRequest missing={missing} objectId={objectId} {...(forThread === undefined ? {} : { forThread })} /> : null;

  return <ErrorState error={error} className={className}
    title={t('access.noAccess.title')} titleAs="h1"
    message={name === null ? t('access.noAccess.kind', { kind: kindName }) : t('access.noAccess.kindNamed', { kind: kindName, name })}
    recovery={{ label: requestable ? t('access.request.title') : t('ui.error.reviewAccess'), content: <>
      <MissingAccessList missing={missing} />
      {way}
      <Button size="sm" variant="ghost" onClick={retry}>{t('access.noAccess.tryAgain')}</Button>
    </> }} />;
}

/**
 * What a screen shows for a node it could not read: the page of what is missing when the server refused
 * (`authz.denied`), and the plain failure with its way to try again for anything else.
 */
export function NodeFailure({ error, onRetry, retrying = false, className }: { error: unknown; onRetry: () => void; retrying?: boolean; className?: string }) {
  if (error instanceof ApiError && error.is('authz.denied')) return <NoAccess error={error} onRetry={onRetry} className={className} />;
  return <ErrorState className={className} error={error} retrying={retrying} onRetry={onRetry} />;
}
