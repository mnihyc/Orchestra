/** Share opened from a selection of several things: the same short form headed "Access to 3 threads", naming the people on each page they may give on, listing the pages it skips with who can change them, and telling each person once about all of them.
 *
 * Naming people on several things is the same change made on each of their pages: one request for each
 * thing, each against the page as it stands. A page the sharer may not give the choice on is listed as skipped, with
 * the people who can, and is left as it was. What was named and what was skipped is said when it is done, thing by
 * thing; one message tells each person about all the things they were named on.
 */
import { useMutation, useQueries } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessTarget, changePage, pageQuery, previewShare, readPage } from '@/api/access';
import { useNodeLabel } from '@/features/navigation/placeLabels';
import { ShareConsequences } from '@/features/access/ShareConsequences';
import { applyShares, canUndoShare, inverseShare, type ShareReview, type ShareResult } from '@/features/access/shareReview';
import type { AccessChoice, PermissionPage } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { DateField } from '@/ui/DateField';
import { InlineAlert } from '@/ui/InlineAlert';
import { Select } from '@/ui/Select';
import { toast } from '@/ui/Toast';

import { accessOption, choiceWords } from './AddHolders';
import { type Candidate, useCandidates, useTargetOrg } from './candidates';
import { keptLine, mayGiveChoice, sharingIsKept } from './gives';
import { HolderView } from './holders';
import { CHOICE_ORDER, nameChange } from './model';
import { tellable, TellThem, type TellState, tellThem, useDirectOff } from './Tell';

type Prepared = { target: AccessTarget; review: ShareReview } | { target: AccessTarget; error: unknown };

function TargetName({ target }: { target: AccessTarget }) {
  const name = useNodeLabel(target.id, target.name);
  return <span className="font-medium wrap-anywhere text-text">{name ?? target.name}</span>;
}

/** The title for what is selected: "Access to 3 threads", or "things" where they are of several kinds. */
function useTitle(targets: readonly AccessTarget[]): string {
  const { t } = useTranslation();
  const kinds = new Set(targets.map((entry) => entry.kind));
  const only = kinds.size === 1 ? [...kinds][0] : undefined;
  switch (only) {
    case 'thread':
      return t('access.several.threads', { count: targets.length });
    case 'topic':
      return t('access.several.topics', { count: targets.length });
    case 'project':
      return t('access.several.projects', { count: targets.length });
    case 'org':
      return t('access.several.orgs', { count: targets.length });
    case 'document':
      return t('access.several.documents', { count: targets.length });
    default:
      return t('access.several.things', { count: targets.length });
  }
}

/** Why a page is skipped, in the reader's own terms, and who can change it. */
function skipReason(t: TFunction, page: PermissionPage, name: string, choice: AccessChoice, kind: string): string {
  if (sharingIsKept(page)) return keptLine(t, page, name);
  const who = page.owners.flatMap((entry) => (entry.holder.kind === 'user' ? [entry.holder.user.display_name] : entry.holder.kind === 'group' ? [entry.holder.group.name] : [])).join(', ');
  const words = choiceWords(t, choice, kind);
  return who === '' ? t('access.several.cannotNobody', { choice: words }) : t('access.several.cannot', { choice: words, who });
}

export function ShareSeveralDialog({ targets, open, onClose }: { targets: readonly AccessTarget[]; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const title = useTitle(targets);
  const first = targets[0];
  const orgId = useTargetOrg(first ?? { kind: 'org', id: '', name: '' });
  const directOff = useDirectOff(first ?? { kind: 'org', id: '', name: '' });
  const pages = useQueries({ queries: targets.map((entry) => pageQuery(entry)) });
  const [query, setQuery] = useState('');
  const [staged, setStaged] = useState<Candidate[]>([]);
  const [wanted, setChoice] = useState<AccessChoice>('full_view');
  const [until, setUntil] = useState('');
  const [tell, setTell] = useState<TellState>({ on: true, line: '' });
  const [active, setActive] = useState(targets);
  const [prepared, setPrepared] = useState<Prepared[] | null>(null);
  const [results, setResults] = useState<ShareResult[] | null>(null);
  const [kept, setKept] = useState<ShareResult[]>([]);
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState<string | null>(null);
  const [readError, setReadError] = useState<unknown>(null);
  const [inverses, setInverses] = useState<{ target: AccessTarget; error?: unknown }[] | null>(null);
  const candidates = useCandidates(query, orgId, { kinds: ['user', 'group', 'role', 'members'], thing: title, placeId: first !== undefined && ['org', 'project', 'topic', 'thread'].includes(first.kind) ? first.id : orgId ?? undefined });

  const loaded = pages.every((entry) => !entry.isPending);
  // The choice stays one that somebody may give on at least one page, and begins on the lowest.
  const may = (choice: AccessChoice) => pages.some((entry) => entry.data !== undefined && mayGiveChoice(entry.data, choice));
  const choice: AccessChoice = may(wanted) ? wanted : (CHOICE_ORDER.find(may) ?? wanted);
  const staging = (entry: Candidate) => staged.some((other) => JSON.stringify(other.input) === JSON.stringify(entry.input));

  const prepare = useMutation({
    mutationFn: async (): Promise<Prepared[]> => {
      const result: Prepared[] = [];
      const expires = until === '' ? null : new Date(`${until}T23:59:59`).toISOString();
      for (const entry of active) {
        try {
          const page = await readPage(entry);
          const change = nameChange(page, choice, staged.map((person) => person.input), { expires_at: expires });
          const preview = await previewShare(entry, { kind: 'page', change });
          result.push({ target: entry, review: { target: entry, before: page, change, preview } });
        } catch (error) {
          result.push({ target: entry, error });
        }
      }
      return result;
    },
    onSuccess: setPrepared,
  });
  const reviews = (prepared ?? []).flatMap((row) => 'review' in row ? [row.review] : []);
  const combined = [...kept, ...(results ?? [])];
  const update = (result: ShareResult) => {
    setKept((before) => before.map((row) => row.review.target.id === result.review.target.id ? result : row));
    setResults((before) => (before ?? []).map((row) => row.review.target.id === result.review.target.id ? result : row));
  };
  const apply = async () => {
    setBusy(true); setResults(reviews.map((review) => ({ review, state: 'queued' })));
    try {
      const result = await applyShares(reviews, update);
      setResults(result);
      const named = result.filter((row) => row.state === 'succeeded').map((row) => row.review.target);
      const told = directOff || !tell.on ? [] : tellable(staged);
      if (named.length > 0 && told.length > 0 && orgId !== null) {
        const refused = await tellThem(named, orgId, told, tell.line, (names) => t('access.tell.wordingMany', { names }));
        if (refused.length > 0) toast({ tone: 'danger', title: t('access.tell.failed', { names: refused.join(', ') }) });
      }
    } finally { setBusy(false); }
  };
  const reconcile = async (result: ShareResult) => {
    setReading(result.review.target.id); setReadError(null);
    try { update({ ...result, state: result.state === 'succeeded' ? 'succeeded' : 'observed', observed: await readPage(result.review.target) }); }
    catch (error) { setReadError(error); } finally { setReading(null); }
  };
  const undo = async () => {
    setBusy(true);
    const output: { target: AccessTarget; error?: unknown }[] = [];
    try { for (const result of combined.filter(canUndoShare)) {
      if (result.after === undefined) continue;
      try { await changePage(result.review.target, inverseShare(result.review.before, result.after)); output.push({ target: result.review.target }); update({ ...result, undone: true }); }
      catch (error) { output.push({ target: result.review.target, error }); }
      setInverses([...output]);
    } } finally { setBusy(false); }
  };
  const failed = (prepared ?? []).filter((row) => 'error' in row);
  const retry = () => {
    setKept(combined.filter((row) => row.state === 'succeeded' || row.state === 'uncertain'));
    setActive([...failed.map((row) => row.target), ...combined.filter((row) => row.state === 'refused' || row.state === 'skipped' || row.state === 'observed').map((row) => row.review.target)]);
    setPrepared(null); setResults(null); prepare.reset();
  };
  const waiting = busy || prepare.isPending;

  const eligible = (page: PermissionPage | undefined) => page !== undefined && mayGiveChoice(page, choice);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !waiting && onClose()}
      size="lg"
      title={title}
      description={t('access.several.lead')}
      dismissible={!waiting}
      footer={
        <>
          <Button disabled={waiting} onClick={onClose}>{t('common.close')}</Button>
          {prepared !== null && results === null && <><Button disabled={waiting} onClick={() => setPrepared(null)}>{t('shareReview.back')}</Button><Button variant="primary" pending={busy} disabled={!reviews.some((row) => row.preview.allowed && row.preview.revision === row.change.expected_revision)} onClick={() => void apply()}>{t('shareReview.apply', { count: reviews.filter((row) => row.preview.allowed && row.preview.revision === row.change.expected_revision).length })}</Button></>}
          {results !== null && combined.some(canUndoShare) && <Button pending={busy} onClick={() => void undo()}>{t('placeReview.undo')}</Button>}
          {results !== null && (failed.length > 0 || results.some((row) => row.state === 'refused' || row.state === 'skipped' || row.state === 'observed')) && <Button disabled={waiting} onClick={retry}>{t('placeReview.reviewAgain')}</Button>}
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        {prepared === null && <div className="flex flex-wrap items-start gap-2">
          {candidates.placePicker}
          <Combobox
            label={t('access.add.label')}
            hideLabel
            placeholder={t('access.add.placeholder')}
            query={query}
            onQueryChange={setQuery}
            minChars={0}
            loading={candidates.loading}
            empty={t('access.candidates.none')}
            groups={candidates.groups.map((group) => ({ ...group, options: group.options.filter((option) => { const found = candidates.byValue.get(option.value); return found === undefined || !staging(found); }) }))}
            fieldClassName="min-w-[12rem] flex-[3_1_14rem]"
            onSelect={(option) => {
              const found = candidates.byValue.get(option.value);
              if (found === undefined) return;
              setStaged((before) => [...before, found]);
              setQuery('');
            }}
          />
          <Select<AccessChoice>
            label={t('access.add.choice')}
            hideLabel
            value={choice}
            onValueChange={setChoice}
            options={CHOICE_ORDER.map((entry) => ({ value: entry, ...accessOption(t, entry, first?.kind ?? 'thread'), disabled: !may(entry) }))}
            fieldClassName="flex-[1_1_9rem]"
          />
          <DateField label={t('access.add.until')} hideLabel value={until} onValueChange={setUntil} aria-label={t('access.add.untilLabel')} fieldClassName="w-[12rem]" />
          <Button variant="primary" icon={<Plus aria-hidden />} pending={prepare.isPending} blocked={staged.length === 0 || !loaded || !may(choice)} onClick={() => prepare.mutate()}>
            {t('shareReview.review')}
          </Button>
        </div>}

        {staged.length > 0 && (
          <ul className="flex flex-wrap items-center gap-2" aria-label={t('access.add.staged')}>
            {staged.map((entry) => (
              <li key={JSON.stringify(entry.input)}>
                <HolderView
                  holder={entry.holder}
                  target={first ?? { kind: 'org', id: '', name: '' }}
                  outside={entry.outside}
                  remove={prepared === null ? { label: t('access.add.unstage', { name: entry.name }), onRemove: () => setStaged((before) => before.filter((other) => other !== entry)) } : undefined}
                />
              </li>
            ))}
          </ul>
        )}
        {prepared === null && staged.length > 0 && <TellThem target={first ?? { kind: 'org', id: '', name: '' }} staged={staged} state={tell} onChange={setTell} />}
        {prepare.error !== null && <ErrorAlert error={prepare.error} />}
        {readError !== null && <ErrorAlert error={readError} />}
        {results !== null && (failed.length > 0 || combined.some((row) => row.state !== 'succeeded')) && <InlineAlert tone="warning">{t('placeReview.partial')}</InlineAlert>}

        <ul aria-label={t('access.several.list')} className="flex flex-col divide-y divide-border rounded-lg border border-border">
          {prepared === null && active.map((entry) => {
            const index = targets.findIndex((target) => target.id === entry.id);
            const page = pages[index]?.data;
            return (
              <li key={entry.id} className="flex flex-col gap-1 px-3 py-2 text-sm">
                <TargetName target={entry} />
                {(
                  pages[index]?.isError === true ? <ErrorAlert error={pages[index].error} /> : page === undefined ? (
                    <span className="text-text-muted">{t('access.several.reading')}</span>
                  ) : eligible(page) ? (
                    <span className="text-text-muted">{t('access.several.willName')}</span>
                  ) : (
                    <span className="text-text-muted">{t('access.several.skipped', { reason: skipReason(t, page, entry.name, choice, entry.kind) })}</span>
                  )
                )}
              </li>
            );
          })}
          {prepared?.filter((row) => 'error' in row || results === null).map((row) => <li key={row.target.id} className="flex flex-col gap-3 p-3"><TargetName target={row.target} />{'review' in row ? <><ShareConsequences preview={row.review.preview} target={row.target} />{row.review.preview.revision !== row.review.change.expected_revision && <InlineAlert tone="warning">{t('shareReview.changed')}</InlineAlert>}</> : <><p>{t('placeReview.skipped')}</p><ErrorAlert error={row.error} /></>}</li>)}
          {results !== null && combined.map((result) => <li key={result.review.target.id} className="flex flex-col gap-2 p-3" aria-live="polite">
            <TargetName target={result.review.target} /><p>{t(`placeReview.${result.state}`)}</p>
            {result.undone === true && <p>{t('placeReview.undone')}</p>}
            {result.state === 'skipped' && <ShareConsequences target={result.review.target} preview={result.review.preview} />}
            {result.error !== undefined && <ErrorAlert error={result.error} />}
            {result.state === 'uncertain' && <InlineAlert tone="warning">{t('placeReview.uncertainAbout')}</InlineAlert>}
            {(result.state === 'uncertain' || result.state === 'refused' || result.state === 'observed') && <Button size="sm" pending={reading === result.review.target.id} onClick={() => void reconcile(result)}>{t('placeReview.readAgain')}</Button>}
            {result.observed !== undefined && <section><p>{t('shareReview.observed', { revision: result.observed.revision })}</p><ul>{result.observed.rows.map((row) => <li key={`${row.permission}:${row.profile}`}><code>{row.permission}</code>{row.entries.map((entry) => <HolderView key={entry.id} holder={entry.holder} target={result.review.target} />)}</li>)}</ul></section>}
          </li>)}
        </ul>
        {inverses !== null && <section aria-label={t('placeReview.undoTitle')}><h3>{t('placeReview.undoTitle')}</h3><ul>{inverses.map((row) => <li key={row.target.id}><TargetName target={row.target} />{row.error === undefined ? <p>{t('placeReview.succeeded')}</p> : <><ErrorAlert error={row.error} /><Button size="sm" pending={reading === row.target.id} onClick={() => { const result = combined.find((result) => result.review.target.id === row.target.id); if (result !== undefined) void reconcile(result); }}>{t('placeReview.readAgain')}</Button></>}</li>)}</ul></section>}
      </div>
    </Dialog>
  );
}
