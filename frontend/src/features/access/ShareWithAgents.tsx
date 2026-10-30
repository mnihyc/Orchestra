/** The dialog that shares a thing with agents: whose agents (threads, or places), what a place covers, the everyday choice with By area behind it, a copy or a share that stays, and a button that says what happens.
 *
 * It is Share with another kind of holder: the same field that finds whom, the same control for a place
 * ("Web and everything in it" or "Only Web itself"), the same three words, Can view, Can edit and Can
 * manage, offered up to what the reader may give and switched off, with the reason, beyond it. A copy
 * is the thing as it is now, written as files into one agent's workspace; a share that stays is read by the agents
 * whenever they work, until it is taken back or ends. Whatever the server refuses is said beside the choice that caused it:
 * a copy over the limit with the size against the limit and the ways on, a topic with the tools off, what the sharer lacks.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Bot, Copy, Infinity as Forever, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';

import { type AccessTarget, isPlaceKind, previewShare } from '@/api/access';
import { ShareConsequences } from '@/features/access/ShareConsequences';
import { ApiError, api, ok } from '@/api/client';
import { ThreadIdentity } from '@/features/people/ThreadIdentity';
import { namesQuery } from '@/api/search';
import { shareWithAgents } from '@/api/share';
import { settingsQuery } from '@/api/settings';
import type { AgentLevel, NameHit, PermissionPage, ShareParts, ShareWithAgents } from '@/generated/api-types';
import { Hint } from '@/features/help/Hint';
import { useDebounced } from '@/lib/debounce';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Combobox, type ComboboxGroup } from '@/ui/Combobox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { DateField } from '@/ui/DateField';
import { InlineAlert } from '@/ui/InlineAlert';
import { RadioGroup } from '@/ui/RadioGroup';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';
import { ByteSize } from '@/ui/ByteSize';

import { type Area, areasOf, LEVELS, levelNeeded, levelOfParts, partsOfLevel, readOnly, stepOf, stepsOf, withStep } from './agentShares';
import { AREA_WORDS, stepOption } from './AgentRow';
import { CoversLine, type PlaceKind, type Reach, ReachSelect } from './Covers';

/** The most threads or places one share names. */
const WHOSE_MAX = 10;

/** Whose agents a share is for. */
interface Whose {
  id: string;
  kind: 'thread' | 'org' | 'project' | 'topic';
  name: string;
}

/** A thread's agent and state, as a chip shows them. */
function ThreadAgent({ id }: { id: string }) {
  return <ThreadIdentity id={id} />;
}

/** The field that finds whose agents: threads, by their agent and state, and places. */
function WhosePicker({ whose, onChange, target }: { whose: Whose[]; onChange: (next: Whose[]) => void; target: AccessTarget }) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const settled = useDebounced(query.trim());
  const found = useQuery({ ...namesQuery({ q: settled, kinds: ['thread', 'topic', 'project', 'org'], limit: 8 }), enabled: settled.length >= 2 });
  const taken = new Set(whose.map((entry) => entry.id));
  const options = (found.data ?? []).filter((hit: NameHit) => !taken.has(hit.id) && hit.id !== target.id);
  const groups: ComboboxGroup<string>[] = [
    {
      id: 'whose',
      label: t('access.agents.dialog.whoseGroup'),
      options: options.map((hit) => ({
        value: hit.id,
        label: hit.name,
        text: hit.name,
        icon: <Bot aria-hidden />,
        description: `${t(`access.kinds.${hit.kind as 'thread'}`)} · ${hit.path.map((place) => place.name ?? '').filter(Boolean).slice(0, -1).join(' / ')}`,
      })),
    },
  ];
  const add = (entry: Whose) => onChange([...whose, entry].slice(0, WHOSE_MAX));
  const place = isPlaceKind(target.kind) ? target.kind : null;

  return (
    <div className="flex flex-col gap-2">
      <Combobox
        label={t('access.agents.dialog.whose')}
        aside={<Hint id="agents-whose" />}
        placeholder={t('access.agents.dialog.whosePlaceholder')}
        query={query}
        onQueryChange={setQuery}
        minChars={2}
        loading={found.isFetching}
        empty={found.isError ? t('access.agents.dialog.namesFailed') : t('access.candidates.none')}
        groups={groups}
        disabled={whose.length >= WHOSE_MAX}
        onSelect={(option) => {
          const hit = (found.data ?? []).find((entry) => entry.id === option.value);
          if (hit === undefined || (hit.kind !== 'thread' && hit.kind !== 'org' && hit.kind !== 'project' && hit.kind !== 'topic')) return;
          add({ id: hit.id, kind: hit.kind, name: hit.name });
          setQuery('');
        }}
      />
      {place !== null && !taken.has(target.id) && (
        <div>
          <Button size="sm" variant="ghost" onClick={() => add({ id: target.id, kind: place, name: target.name })}>
            {t('access.agents.dialog.itself', { kind: t(`access.kinds.${place}`) })}
          </Button>
        </div>
      )}
      {whose.length > 0 && (
        <ul aria-label={t('access.agents.dialog.chosen')} className="flex flex-col gap-1">
          {whose.map((entry) => (
            <li key={entry.id} className="flex flex-wrap items-center gap-2 rounded-md border border-border bg-surface px-2 py-1 text-sm">
              <Bot className="shrink-0 text-text-muted" size={16} aria-hidden />
              <span className="min-w-0 font-medium wrap-anywhere text-text">{t('access.holder.agentsOf', { place: entry.name })}</span>
              {entry.kind === 'thread' && <ThreadAgent id={entry.id} />}
              <button
                type="button"
                aria-label={t('access.agents.dialog.remove', { name: entry.name })}
                className="ml-auto inline-flex size-6 shrink-0 items-center justify-center rounded-full text-text-muted hover:bg-bg-muted hover:text-text"
                onClick={() => onChange(whose.filter((other) => other.id !== entry.id))}
              >
                <X size={16} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** What the words of the primary button are: what happens when it is pressed. */
function buttonWords(t: TFunction, mode: 'live' | 'snapshot', whose: readonly Whose[], target: AccessTarget, parts: ShareParts, areas: readonly Area[]): string {
  const only = whose.length === 1 ? whose[0] : undefined;
  if (whose.length === 0) return t('access.agents.dialog.button.none');
  if (mode === 'snapshot') {
    return only === undefined ? t('access.agents.dialog.button.copyMany', { count: whose.length }) : t('access.agents.dialog.button.copyOne', { name: only.name });
  }
  const who =
    only === undefined
      ? t('access.agents.dialog.button.agents', { count: whose.length })
      : only.id === target.id
        ? t('access.agents.dialog.button.thisAgents', { kind: t(`access.kinds.${only.kind}`) })
        : t('access.agents.dialog.button.ofAgents', { name: only.name });
  const shared = areas.filter((area) => stepOf(area, parts) > 0);
  // What is shared by name: the documents where nothing else is, and otherwise the thing itself.
  const what = shared.length === 1 && shared[0] === 'documents' ? t('access.agents.dialog.button.documents') : t('access.agents.dialog.button.this', { name: target.name });
  const top = Math.max(...areas.map((area) => stepOf(area, parts)), 0);
  // Areas at different levels have no one verb: the button says what is shared and with whom, and the levels are above it.
  if (new Set(shared.map((area) => stepOf(area, parts))).size > 1) return t('access.agents.dialog.button.mixed', { who, what });
  if (top >= 3) return t('access.agents.dialog.button.manage', { who, what });
  if (top === 2) return t(shared.length === 1 && shared[0] === 'documents' ? 'access.agents.dialog.button.edit' : 'access.agents.dialog.button.write', { who, what });
  return t('access.agents.dialog.button.read', { who, what });
}

/** What the refusals of a share say beside the choice they are about. */
function Refused({ error, onRaise, raising }: { error: unknown; onRaise: ((scopeId: string) => void) | null; raising: boolean }) {
  const { t } = useTranslation();
  if (error instanceof ApiError && error.is('share.too_large')) {
    const extra = error.problem as unknown as { bytes?: number; limit?: number; scope?: { id: string; name: string | null } | null };
    return (
      <InlineAlert tone="warning" title={t('access.agents.dialog.tooLarge')}>
        <p>
          {extra.bytes !== undefined && extra.limit !== undefined ? (
            <>
              <ByteSize bytes={extra.bytes} /> {t('access.agents.dialog.of')} <ByteSize bytes={extra.limit} />.{' '}
            </>
          ) : null}
          {t('access.agents.dialog.tooLargeWays')}
        </p>
        {onRaise !== null && extra.scope !== null && extra.scope !== undefined && (
          <Button size="sm" pending={raising} onClick={() => onRaise(extra.scope?.id ?? '')}>
            {t('access.agents.dialog.raise', { scope: extra.scope.name ?? t('access.place.hidden') })}
          </Button>
        )}
      </InlineAlert>
    );
  }
  if (error instanceof ApiError && error.is('share.unavailable')) {
    const extra = error.problem as unknown as { reason?: string; thread?: { name: string | null } | null };
    const name = extra.thread?.name ?? t('access.place.hidden');
    return <InlineAlert tone="warning">{t(extra.reason === 'no_tools' ? 'access.agents.dialog.noTools' : 'access.agents.dialog.toolsOff', { name })}</InlineAlert>;
  }
  return <ErrorAlert error={error} />;
}

export function ShareWithAgentsDialog({ target, page, open, onClose, initialRecipient }: { target: AccessTarget; page: PermissionPage; open: boolean; onClose: () => void; initialRecipient?: { id: string; name: string } }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const gives = page.gives.agents;
  const place: PlaceKind | null = isPlaceKind(target.kind) ? target.kind : null;

  // A refusal appears below what the dialog shows when it is tall: it is brought into view, since it is what the press of the button was answered with.
  const refusal = useRef<HTMLDivElement>(null);
  const [whose, setWhose] = useState<Whose[]>(() => initialRecipient === undefined ? [] : [{ ...initialRecipient, kind: 'thread' }]);
  const [reach, setReach] = useState<Reach>('inside');
  const [wantedLevel, setLevel] = useState<AgentLevel>('view');
  const [byArea, setByArea] = useState(false);
  const [custom, setCustom] = useState<ShareParts>({});
  const [mode, setMode] = useState<'live' | 'snapshot'>('live');
  const [comment, setComment] = useState('');
  const [until, setUntil] = useState('');
  const [download, setDownload] = useState(true);
  const [readFirst, setReadFirst] = useState(false);

  const areas = areasOf(target.kind, reach);
  const everyThread = whose.length > 0 && whose.every((entry) => entry.kind === 'thread');
  const copy = mode === 'snapshot' && everyThread;
  // The level begins as the lowest the reader may give, and stays one they may; a copy is read only.
  const level: AgentLevel = copy ? 'view' : gives.includes(wantedLevel) ? wantedLevel : (LEVELS.find((entry) => gives.includes(entry)) ?? wantedLevel);
  const chosen: ShareParts = byArea ? custom : partsOfLevel(level, areas);
  const parts: ShareParts = copy ? readOnly(chosen) : chosen;
  const nothing = areas.every((area) => stepOf(area, parts) === 0);
  const asLevel = levelOfParts(parts, areas);

  const body = {
    what: target.kind === 'chat' ? { chat_id: target.id } : target.kind === 'document' ? { document_id: target.id } : { node_id: target.id },
    mode: copy ? ('snapshot' as const) : ('live' as const),
    to: everyThread ? whose.map((entry) => ({ thread_id: entry.id })) : whose.map((entry) => ({ node_id: entry.id })),
    parts,
    ...(parts.documents !== null && parts.documents !== undefined ? { read_first: readFirst } : {}),
    ...(place !== null && !copy ? { inside: reach === 'inside' } : {}),
    ...(copy && comment.trim() !== '' ? { comment: comment.trim() } : {}),
    ...(!copy && until !== '' ? { expires_at: new Date(`${until}T23:59:59`).toISOString() } : {}),
    // The server that knows the tick takes it; it is told only when it was unticked, which is the one thing it needs to be told.
    ...(!download && !copy ? { download: false } : {}),
  };

  const review = useQuery({
    queryKey: ['preview', target.kind, target.id, 'share-agents', body],
    queryFn: async () => {
      const frozen = { ...body, client_id: actKey(body) } satisfies ShareWithAgents;
      return { body: frozen, preview: await previewShare(target, { kind: 'agents', share: frozen }) };
    },
    enabled: open && whose.length > 0 && !nothing, retry: false, gcTime: 0, staleTime: 0,
  });
  const share = useMutation({
    mutationFn: () => {
      if (review.data === undefined || !review.data.preview.allowed || review.isFetching || review.isError) throw new Error(t('placeReview.newReviewRequired'));
      return shareWithAgents(review.data.body);
    },
    onSuccess: () => {
      toast({ tone: 'success', title: t(copy ? 'access.agents.dialog.copied' : 'access.agents.dialog.shared', { count: whose.length }) });
      onClose();
    },
  });

  const failure = share.error ?? review.error;
  useEffect(() => {
    if (failure !== null) refusal.current?.scrollIntoView({ block: 'nearest' });
  }, [failure]);

  // Raising the limit of copies where it is set, for whoever may change it there, without leaving the dialog.
  const limitOf = useQuery({ ...settingsQuery(failure instanceof ApiError && failure.is('share.too_large') ? ((failure.problem as unknown as { scope?: { id: string } }).scope?.id ?? target.id) : target.id), enabled: failure instanceof ApiError && failure.is('share.too_large') });
  const writable = limitOf.data?.find((entry) => entry.key === 'sharing.snapshot_max_bytes')?.writable === true;
  const raise = useMutation({
    mutationFn: async (scopeId: string) => {
      const bytes = failure instanceof ApiError ? ((failure.problem as unknown as { bytes?: number }).bytes ?? 0) : 0;
      await ok(api.PUT('/nodes/{id}/settings/{key}', { params: { path: { id: scopeId, key: 'sharing.snapshot_max_bytes' } }, body: { value: bytes } }));
    },
    onSuccess: () => share.reset(),
  });

  const levelOff = (entry: AgentLevel) => !gives.includes(entry) || (copy && entry !== 'view');
  const levelReason = (entry: AgentLevel): string | undefined => (!gives.includes(entry) ? t('access.agents.dialog.notYours') : copy && entry !== 'view' ? t('access.agents.dialog.copyReadOnly') : undefined);
  const mayCopy = everyThread;
  const stepOff = (step: number) => {
    const needed = levelNeeded(step);
    return needed !== null && !gives.includes(needed);
  };
  const documentsEdited = areas.includes('documents') && stepOf('documents', parts) >= 2;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !share.isPending && onClose()}
      size="lg"
      title={t('access.agents.dialog.title', { name: target.name })}
      description={t('access.agents.dialog.lead')}
      dismissible={!share.isPending}
      footer={
        <>
          <Button disabled={share.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" icon={copy ? <Copy aria-hidden /> : <Bot aria-hidden />} pending={share.isPending} blocked={whose.length === 0 || nothing || review.isFetching || review.isError || review.data?.preview.allowed !== true} onClick={() => share.mutate()}>
            {buttonWords(t, copy ? 'snapshot' : 'live', whose, target, parts, areas)}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <WhosePicker whose={whose} onChange={setWhose} target={target} />

        {place !== null && (
          <div className="flex flex-col gap-2">
            <ReachSelect place={target.name} value={reach} onValueChange={setReach} disabled={copy} />
            <CoversLine page={page} kind={place} name={target.name} reach={reach} whom="agents" />
          </div>
        )}

        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-end gap-2">
            <Select<AgentLevel>
              label={t('access.agents.dialog.what')}
              value={level}
              onValueChange={setLevel}
              disabled={byArea}
              fieldClassName="min-w-[12rem] flex-[1_1_12rem]"
              help={<Hint id="agents-areas" />}
              options={LEVELS.map((entry) => ({ value: entry, ...choiceOption(t, `accessLevel.${entry}`), disabled: levelOff(entry), ...(levelReason(entry) === undefined ? {} : { description: levelReason(entry) }) }))}
            />
            <Button size="sm" variant="ghost" aria-pressed={byArea} onClick={() => { setCustom(chosen); setByArea((on) => !on); }}>
              {t('access.agents.dialog.byArea')}
            </Button>
          </div>
          {level === 'manage' && !byArea && (
            <p className="flex items-start gap-1 text-sm text-text-muted">
              <span>{t('access.agents.manageNote')}</span>
              <Hint id="agents-manage" />
            </p>
          )}
          {byArea && (
            <div role="group" aria-label={t('access.agents.dialog.byAreaLabel')} className="flex flex-wrap gap-x-4 gap-y-2">
              {areas.map((area) => (
                <Select<string>
                  key={area}
                  label={t(AREA_WORDS[area])}
                  size="sm"
                  value={String(stepOf(area, custom))}
                  onValueChange={(next) => setCustom(withStep(custom, area, Number(next)))}
                  fieldClassName="min-w-[9rem] flex-[1_1_9rem]"
                  options={stepsOf(area).map((step) => ({ value: String(step), ...stepOption(t, area, step), disabled: stepOff(step) || (copy && step > 1) }))}
                />
              ))}
            </div>
          )}
          {byArea && asLevel === null && !nothing && <p className="text-sm text-text-muted">{t('access.agents.dialog.custom')}</p>}
          {!byArea && gives.length === 0 && <EmptyState compact title={t('access.agents.cannotShare', { name: target.name })} />}
        </div>

        <RadioGroup<'live' | 'snapshot'>
          label={t('access.agents.dialog.kind')}
          value={mode}
          onValueChange={setMode}
          options={[
            {
              value: 'live',
              ...choiceOption(t, 'agentShareMode.live'),
              icon: <Forever aria-hidden />,
            },
            {
              value: 'snapshot',
              ...choiceOption(t, 'agentShareMode.snapshot'),
              icon: <Copy aria-hidden />,
              ...(mayCopy ? {} : { disabled: t('access.agents.dialog.copyThreadsOnly') }),
            },
          ]}
        />

        {copy ? (
          <Textarea label={t('access.agents.dialog.message')} optional hint={t('access.agents.dialog.messageHint')} value={comment} onChange={(event) => setComment(event.target.value)} />
        ) : (
          <div className="flex flex-col gap-2">
            <DateField label={t('access.agents.dialog.until')} optional value={until} onValueChange={setUntil} fieldClassName="w-[12rem]" />
            <Checkbox label={t('people.allowExport')} help={<Hint id="agents-download" />} checked={download} onCheckedChange={setDownload} />
          </div>
        )}

        <div className="flex flex-col gap-1 text-sm text-text-muted">
          <p>{t('access.agents.repeat')}</p>
          {parts.documents !== null && parts.documents !== undefined && <Checkbox label={t('people.readFirst')} hint={t('people.readFirstHint')} checked={readFirst} onCheckedChange={setReadFirst} />}
          {documentsEdited && <p>{t('access.agents.dialog.history')}</p>}
        </div>

        {whose.length > 0 && !nothing && (review.isPending || review.isFetching ? <SkeletonRows rows={3} /> : review.isError ? <Button onClick={() => void review.refetch()}>{t('actions.refresh.label')}</Button> : <ShareConsequences preview={review.data.preview} target={target} />)}
        {failure !== null && (
          <div ref={refusal}>
            <Refused error={failure} onRaise={writable ? (scopeId) => raise.mutate(scopeId) : null} raising={raise.isPending} />
          </div>
        )}
        {raise.error !== null && <ErrorAlert error={raise.error} />}
      </div>
    </Dialog>
  );
}
