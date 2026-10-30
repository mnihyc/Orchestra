/** The dialog that ends a membership: what that person owned alone and where each thing goes, what they owned with others, and what becomes of each resource.
 *
 * Ending a membership always ends the membership of the whole tree: somebody who is only to change
 * department is moved instead. Before anything happens the dialog lists what is that person's in the
 * tree and what will happen to each part of it: what they own alone passes to the first owner of the
 * org it lies in, what they own with others stays with the others, and each resource they made is
 * kept, disabled or removed as chosen. The last owner of a root org is told to name another owner first.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { ApiError } from '@/api/client';
import { holdingsQuery, removeMember } from '@/api/directory';
import type { Member, Org, ResourceFate } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SegmentedControl, type Segment } from '@/ui/SegmentedControl';
import { ChoiceFieldHelp } from '@/ui/ChoiceField';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

export interface EndMembershipProps {
  /** The org the list was opened on. */
  org: Org;
  member: Member;
  /** Whether the person ending it is the member: the words are then "Leave" and not "Remove". */
  self: boolean;
  onClose: () => void;
}

export function EndMembership({ org, member, self, onClose }: EndMembershipProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const root = org.path[0]?.name ?? org.name;
  const name = member.user.display_name;
  const holdings = useQuery(holdingsQuery(org.id, self ? 'me' : member.user.id));
  // A resource not chosen about is kept: the safest thing to do with what others may use.
  const [fates, setFates] = useState<Record<string, ResourceFate>>({});

  const end = useMutation({
    mutationFn: () => removeMember(org.id, self ? 'me' : member.user.id, fates),
    onSuccess: () => {
      toast({ tone: 'success', title: self ? t('settings.members.end.leftDone', { root }) : t('settings.members.end.done', { name, root }) });
      onClose();
    },
  });
  const lastOwner = end.error instanceof ApiError && end.error.is('owner.last');

  const data = holdings.data;
  const owned = data?.nodes;
  const ownedParts =
    owned === undefined
      ? []
      : [
          owned.orgs > 0 ? t('settings.members.end.nodes.orgs', { count: owned.orgs }) : null,
          owned.projects > 0 ? t('settings.members.end.nodes.projects', { count: owned.projects }) : null,
          owned.topics > 0 ? t('settings.members.end.nodes.topics', { count: owned.topics }) : null,
          owned.threads > 0 ? t('settings.members.end.nodes.threads', { count: owned.threads }) : null,
          owned.documents > 0 ? t('settings.members.end.nodes.documents', { count: owned.documents }) : null,
        ].filter((part): part is string => part !== null);
  // What is owned with others, by kind, documents among it; the server's total stays the fallback.
  const sharedKinds = data?.shared_by_kind;
  const sharedParts =
    sharedKinds === undefined
      ? []
      : [
          sharedKinds.orgs > 0 ? t('settings.members.end.nodes.orgs', { count: sharedKinds.orgs }) : null,
          sharedKinds.projects > 0 ? t('settings.members.end.nodes.projects', { count: sharedKinds.projects }) : null,
          sharedKinds.topics > 0 ? t('settings.members.end.nodes.topics', { count: sharedKinds.topics }) : null,
          sharedKinds.threads > 0 ? t('settings.members.end.nodes.threads', { count: sharedKinds.threads }) : null,
          sharedKinds.documents > 0 ? t('settings.members.end.nodes.documents', { count: sharedKinds.documents }) : null,
          sharedKinds.resources > 0 ? t('settings.members.end.nodes.resources', { count: sharedKinds.resources }) : null,
        ].filter((part): part is string => part !== null);
  const list = (parts: readonly string[]) => new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' }).format(parts);
  const fateOptions: Segment<ResourceFate>[] = [
    { value: 'keep', ...choiceOption(t, 'membershipFate.keep') },
    { value: 'disable', ...choiceOption(t, 'membershipFate.disable') },
    { value: 'remove', ...choiceOption(t, 'membershipFate.remove') },
  ];

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !end.isPending && onClose()}
      title={self ? t('settings.members.end.leaveTitle', { root }) : t('settings.members.end.title', { name, root })}
      description={t('settings.members.end.lead', { root })}
      size="lg"
      dismissible={!end.isPending}
      footer={
        <>
          <Button disabled={end.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="danger" pending={end.isPending} blocked={data === undefined || lastOwner} onClick={() => end.mutate()}>
            {self ? t('settings.members.end.leave', { root }) : t('settings.members.end.submit', { name, root })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2 text-base text-text">
        {holdings.isPending && (
          <SkeletonGroup className="flex flex-col gap-2">
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-4 w-2/3" />
          </SkeletonGroup>
        )}
        {holdings.isError && <ErrorState compact error={holdings.error} retrying={holdings.isRefetching} onRetry={() => void holdings.refetch()} />}
        {data !== undefined && (
          <>
            <p>
              {ownedParts.length === 0
                ? t('settings.members.end.ownsNothing', { name })
                : t('settings.members.end.ownsAlone', { name, what: list(ownedParts) })}
            </p>
            {data.shared > 0 &&
              (sharedParts.length > 0 ? (
                <p>{t('settings.members.end.ownsSharedKinds', { name, what: list(sharedParts) })}</p>
              ) : (
                <p>{t('settings.members.end.ownsShared', { name, count: data.shared })}</p>
              ))}
            {data.resources.length > 0 && (
              <section aria-label={t('settings.members.end.resources', { name })} className="flex flex-col gap-3">
                <h3 className="text-md font-semibold">{t('settings.members.end.resources', { name })}</h3>
                <p className="text-sm text-text-muted">{t('settings.members.end.resourcesLead', { name })}</p>
                <ul className="flex flex-col divide-y divide-border rounded-lg border border-border">
                  {data.resources.map((resource) => (
                    <li key={resource.object.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-3 py-2">
                      <div className="min-w-0 flex-1 basis-[min(100%,12rem)]">
                        <div className="flex min-w-0 items-center gap-1"><p className="font-medium wrap-anywhere">{resource.object.name}</p>
                          <ChoiceFieldHelp label={t('settings.members.end.fate', { name: resource.object.name })} options={fateOptions} /></div>
                        <p className="text-sm text-text-muted">
                          {t('settings.members.end.usedByPeople', { count: resource.shared_with })} · {t('settings.members.end.usedByThreads', { count: resource.threads })}
                        </p>
                        {resource.personal_sign_in && <p className="text-sm text-text">{t('settings.members.end.signIn', { name })}</p>}
                      </div>
                      <SegmentedControl
                        label={t('settings.members.end.fate', { name: resource.object.name })}
                        size="sm"
                        value={fates[resource.object.id] ?? 'keep'}
                        onValueChange={(fate) => setFates((before) => ({ ...before, [resource.object.id]: fate }))}
                        segments={fateOptions}
                      />
                    </li>
                  ))}
                </ul>
              </section>
            )}
            {data.machine_registrations.length > 0 && (
              <p>
                {t('settings.members.end.machines', { name })}{' '}
                {list(data.machine_registrations.map((registration) => registration.object.name))} {t('settings.members.end.machinesLeave')}
              </p>
            )}
            {(data.entries > 0 || data.pending > 0) && (
              <p className="text-sm text-text-muted">
                {data.entries > 0 && t('settings.members.end.entries', { count: data.entries, name })}
                {data.pending > 0 && `${data.entries > 0 ? ' ' : ''}${t('settings.members.end.pending', { count: data.pending, name })}`}
              </p>
            )}
          </>
        )}
        {lastOwner && <InlineAlert tone="danger">{t('settings.members.end.lastOwner', { root })}</InlineAlert>}
        {end.error !== null && !lastOwner && <ErrorAlert error={end.error} />}
      </div>
    </Dialog>
  );
}
