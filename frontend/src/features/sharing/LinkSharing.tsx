/** One explicit composer choice; fresh native trials precede existing gift writes and preserve a failed draft. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { api, client, ok, ReceiptAcknowledged } from '@/api/client';
import { meStateKey, meStateQuery } from '@/api/personal';
import { settingsKey, settingsQuery } from '@/api/settings';
import { grantPerson } from '@/api/personActions';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { beginShareAttempt, shareWithAgents, type ShareAttempt } from '@/api/share';
import type { NodeRef, ResolvedSetting, UserRef } from '@/generated/api-types';
import { KnownNodeChip, NamedChip } from '@/features/people/NamedChip';
import { useFormatContext } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatNumber } from '@/lib/format';
import { linkTo } from '@/lib/links';
import { newActKey } from '@/lib/requests';
import { AvatarStack, PeopleCount } from '@/ui/AvatarStack';
import { Button } from '@/ui/Button';
import { DetailCard } from '@/ui/DetailCard';
import { ErrorAlert } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { Switch } from '@/ui/Switch';
import { UserChip } from '@/ui/UserChip';
import { carriedLinks, LinkAudienceChanged, linkPlanSignature, previewLinkPlan, type LinkContext, type LinkParts } from './linkPlan';
import { linkSharingPreference } from './preference';

export function linkPolicy(rows: readonly ResolvedSetting[]): { on: boolean | null; lockedOff: boolean } {
  const row = rows.find((entry) => entry.key === 'sharing.on_link');
  return { on: typeof row?.value === 'boolean' ? row.value : null, lockedOff: row?.value === false && (row.locked_by !== null || row.own?.locked === true) };
}

interface Feedback { owner: string; attempt: string; done: boolean; people: UserRef[]; agents: NodeRef[]; error: unknown }

export function useLinkSharing(context: LinkContext, parts: LinkParts, visible = true) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const user = useStore(client.session, (state) => state.userId);
  const version = useStore(client.session, (state) => state.readerVersion);
  const owner = JSON.stringify([user, version, context.origin, context.version]);
  const latestOwner = useRef(owner);
  const refs = useMemo(() => carriedLinks(parts), [parts]);
  const refsKey = JSON.stringify(refs.map(({ kind, id, path }) => [kind, id, path]));
  const signature = JSON.stringify([owner, parts, context.extraAgents?.map((agent) => agent.id)]);
  const latest = useRef(signature);
  useLayoutEffect(() => { latest.current = signature; latestOwner.current = owner; }, [signature, owner]);
  const held = useRef(new Map<string, string>());
  const agentAttempts=useRef(new Map<string,ShareAttempt>());
  const keyFor = (key: string) => {
    const scoped = `${owner}:${key}`;
    const previous = held.current.get(scoped);
    if (previous !== undefined) return previous;
    const value = newActKey(); held.current.set(scoped, value); return value;
  };
  const settled = useDebounced(refsKey);
  const queryKey = ['link-sharing', owner, settled, context.extraAgents?.map((agent) => agent.id)] as const;
  const hasRefs = refs.length > 0;
  const preview = useQuery({ queryKey, enabled: visible && hasRefs && settled === refsKey && user !== null,
    queryFn: ({ signal }) => previewLinkPlan(context, refs, parts, keyFor, signal), retry: false, staleTime: 0 });
  const place = preview.data?.placeId ?? context.placeId;
  const personal = useQuery({ ...meStateQuery, enabled: visible && hasRefs });
  const policy = useQuery({ ...settingsQuery(place), enabled: visible && hasRefs && place !== '' });
  const preference = linkSharingPreference(personal.data);
  const defaults = linkPolicy(policy.data ?? []);
  const defaultChecked = defaults.on === true && preference === true;
  const readyDefaults = personal.isSuccess && policy.isSuccess && preference !== null && defaults.on !== null;
  const [choice, setChoice] = useState<{ owner: string; on: boolean } | null>(null);
  const skipped = choice?.owner === owner && !choice.on;
  const checked = !skipped && readyDefaults && !defaults.lockedOff && (choice?.owner === owner ? choice.on : defaultChecked);
  const [busy, setBusy] = useState(false);
  const active = useRef(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const currentFeedback = feedback?.owner === owner ? feedback : null;
  const plan = settled === refsKey && preview.isSuccess && !preview.isFetching ? preview.data : undefined;
  const blocked = busy || hasRefs && !skipped && (!readyDefaults || checked && plan === undefined);
  const fail = (error: unknown) => setFeedback({ owner, attempt: signature, done: false, people: [], agents: [], error });

  const send = async (submit: () => void, valid: () => boolean = () => true): Promise<boolean> => {
    if (active.current || blocked) return false;
    if (!hasRefs || skipped || !checked) {
      if (!valid()) return false;
      submit(); setChoice(null); held.current.clear(); agentAttempts.current.clear();
      setFeedback((before) => before?.owner === owner && before.attempt === signature && !before.done ? { ...before, done: true, error: null } : null);
      return true;
    }
    if (plan === undefined) return false;
    active.current = true; setBusy(true);
    const reader = libraryReader(); const started = signature;
    const previous = currentFeedback?.attempt === signature && !currentFeedback.done ? currentFeedback : null;
    const people = new Map<string, UserRef>(previous?.people.map((person) => [person.id, person] as const));
    const agents = new Map<string, NodeRef>(previous?.agents.map((agent) => [agent.id, agent] as const));
    const current = () => sameLibraryReader(reader) && latest.current === started && valid();
    try {
      const controller = new AbortController();
      const fresh = await previewLinkPlan(context, refs, parts, keyFor, controller.signal);
      const [state, rows] = await Promise.all([
        ok(api.GET('/me/state', { signal: controller.signal })),
        ok(api.GET('/nodes/{id}/settings', { params: { path: { id: fresh.placeId } }, signal: controller.signal })),
      ]);
      if (!current()) throw new LinkAudienceChanged();
      cache.setQueryData(meStateKey, state); cache.setQueryData(settingsKey(fresh.placeId), rows);
      const freshPreference = linkSharingPreference(state);
      const freshPolicy = linkPolicy(rows);
      if (freshPreference === null || freshPolicy.on === null || freshPolicy.lockedOff || choice?.owner !== owner && !(freshPolicy.on && freshPreference)
        || linkPlanSignature(fresh) !== linkPlanSignature(plan)) {
        cache.setQueryData(queryKey, fresh);
        throw new LinkAudienceChanged();
      }
      for (const gift of fresh.gifts) {
        if (!current()) throw new LinkAudienceChanged();
        try {
          if (gift.kind === 'person') await grantPerson(gift.target, gift.person.id, gift.choice, false, keyFor(gift.key), context.origin);
          else {
            const scoped=`${owner}:${gift.key}`;
            let attempt=agentAttempts.current.get(scoped);
            if(attempt===undefined) {attempt=beginShareAttempt(keyFor(gift.key));agentAttempts.current.set(scoped,attempt);}
            await shareWithAgents(gift.share,attempt);
          }
        } catch (error) { if (!(error instanceof ReceiptAcknowledged)) throw error; }
        // A confirmed gift gets no replay key on a later, genuinely new grant after revocation.
        held.current.delete(`${owner}:${gift.key}`);
        agentAttempts.current.delete(`${owner}:${gift.key}`);
        if (gift.kind === 'person') people.set(gift.person.id, gift.person);
        else agents.set(gift.agent.id, gift.agent);
      }
      if (!current()) throw new LinkAudienceChanged();
      setFeedback({ owner, attempt: signature, done: true, people: [...people.values()], agents: [...agents.values()], error: null });
      submit(); setChoice(null); held.current.clear(); agentAttempts.current.clear();
      void cache.invalidateQueries({ queryKey: ['person-actions'] });
      void cache.invalidateQueries({ queryKey: ['link-sharing'], refetchType: 'none' });
      return true;
    } catch (error) {
      if (sameLibraryReader(reader) && latestOwner.current === owner) setFeedback({ owner, attempt: signature, done: false, people: [...people.values()], agents: [...agents.values()], error });
      return false;
    } finally { active.current = false; setBusy(false); }
  };
  return { hasRefs, checked, busy, blocked, plan, readyDefaults, lockedOff: defaults.lockedOff,
    error: currentFeedback?.error ?? (hasRefs && !skipped ? personal.error ?? policy.error ?? preview.error
      ?? (personal.isSuccess && preference === null || policy.isSuccess && defaults.on === null ? new Error(t('linkSharing.unavailable')) : null) : null),
    sharedPeople: currentFeedback?.people ?? [], sharedAgents: currentFeedback?.agents ?? [], send, fail,
    change: (on: boolean) => { setChoice({ owner, on }); setFeedback((before) => before?.owner === owner ? { ...before, error: null } : before); },
    retry: () => { void Promise.all([personal.refetch(), policy.refetch(), preview.refetch()]); },
  };
}

type Sharing = ReturnType<typeof useLinkSharing>;

function AgentCount({ agents, label }: { agents: readonly NodeRef[]; label: string }) {
  const { t } = useTranslation();
  return agents.length === 0 ? <span>{label}</span> : <DetailCard label={label} content={() => <ul className="flex min-w-0 flex-col gap-1 p-3">{agents.map((agent) => <li key={agent.id}>{agent.redacted || agent.name === null ? <span className="text-sm text-text-muted">{t('people.privateThread')}</span> : <KnownNodeChip node={agent} />}</li>)}</ul>}>
    <button type="button" className="focus-inset rounded-sm text-sm text-text-muted underline decoration-dotted underline-offset-2">{label}</button>
  </DetailCard>;
}

export function LinkSharing({ sharing }: { sharing: Sharing }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const plan = sharing.plan;
  const people = t('linkSharing.people', { count: plan?.people.length ?? 0, number: formatNumber(plan?.people.length ?? 0, locale) });
  const agents = t('linkSharing.agents', { count: plan?.agents.length ?? 0, number: formatNumber(plan?.agents.length ?? 0, locale) });
  const label = t('linkSharing.label', { people, agents });
  const hasResult = sharing.sharedPeople.length + sharing.sharedAgents.length > 0;
  if (!sharing.hasRefs && !hasResult) return null;
  return <div className="flex min-w-0 flex-col gap-1 text-sm text-text-muted" data-link-sharing="">
    {sharing.hasRefs && (sharing.lockedOff ? <span>{t('linkSharing.lockedOff')}</span> : !sharing.readyDefaults || plan === undefined ? <span className="flex flex-wrap items-center gap-2">
      <span role="status">{(sharing.error === null || sharing.error === undefined) ? t('linkSharing.checking') : t('linkSharing.unavailable')}</span>
      <Button size="sm" variant="ghost" disabled={sharing.busy} onClick={() => sharing.change(false)}>{t('linkSharing.skip')}</Button>
    </span> : plan.gifts.length > 0 ? <>
      <div className="flex min-w-0 flex-wrap items-center gap-1">
        <Switch label={<span className="sr-only">{label}</span>} checked={sharing.checked} disabled={sharing.busy} onCheckedChange={sharing.change} />
        <span>{t('linkSharing.lead')}</span>
        {plan.people.length === 0 ? <span>{people}</span> : <PeopleCount users={plan.people}>{people}</PeopleCount>}
        <span aria-hidden>{t('linkSharing.separator')}</span><AgentCount agents={plan.agents} label={agents} />
      </div>
      {plan.memberships.length > 0 && <div className="flex flex-wrap items-center gap-1"><span>{t('linkSharing.membership')}</span>{plan.memberships.map((target) => <NamedChip key={target.id} id={target.id} kind="chat" path={linkTo(target)} original={target.name} reference={{ kind: 'chat', id: target.id }} />)}</div>}
    </> : null)}
    {sharing.hasRefs && plan?.unknown === true && <span>{t('linkSharing.review')}</span>}
    {sharing.error instanceof LinkAudienceChanged ? <StateLine role="alert" tone="warning" compact message={t('linkSharing.changed')}
      action={<Button size="sm" onClick={sharing.retry}>{t('common.tryAgain')}</Button>} />
      : sharing.error !== null && sharing.error !== undefined && <ErrorAlert error={sharing.error} action={<Button size="sm" onClick={sharing.retry}>{t('common.tryAgain')}</Button>} />}
    {hasResult && <div role="status" className="flex min-w-0 flex-wrap items-center gap-1"><span>{t('linkSharing.shared')}</span>
      {sharing.sharedPeople.length === 1 && sharing.sharedPeople[0] !== undefined ? <UserChip user={sharing.sharedPeople[0]} /> : <AvatarStack users={sharing.sharedPeople} />}
      {sharing.sharedAgents.length > 0 && <AgentCount agents={sharing.sharedAgents} label={t('linkSharing.agents', { count: sharing.sharedAgents.length, number: formatNumber(sharing.sharedAgents.length, locale) })} />}
    </div>}
  </div>;
}
