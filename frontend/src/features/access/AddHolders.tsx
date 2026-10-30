/** The field that adds people, groups and roles to a page with one of the three choices and an optional end, and says what that comes to before it is done.
 *
 * It offers only what its reader may give: the choices, and on a place the roles, are those the page says they may give.
 * What is chosen from the list waits as a chip under the field until Add is pressed, so that several
 * are named in one request (`name`, in every row the choice covers). On a place the field first says
 * what is shared: the place with everything in it, which gives the role that goes with the choice and
 * lists the person under People, or the place alone, which names them on its own page and opens nothing inside.
 * The menu of choices lists what the page says the reader may give and shows the rest switched off, with the
 * one line that says why. Where the place keeps sharing to owners the field is switched off, with who can.
 * Somebody from outside is offered where the caller may name one and shown disabled, with who can, where
 * not; an address nobody is registered with leads to an invitation to this one place. Naming somebody on
 * a thread that runs on a person's machine says what that includes, before it is done.
 */
import { useMutation } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Mail, Plus } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { hintOf } from '@/features/help/hints';
import { lineHintId } from './hints';

import { type AccessTarget, changePage, giveRole, isPlaceKind, readPage, requestAccess, roleSubject } from '@/api/access';
import { ApiError } from '@/api/client';
import type { AccessChoice, PermissionPage, Role } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Combobox, type ComboboxGroup } from '@/ui/Combobox';
import { ErrorAlert } from '@/ui/ErrorState';
import { DateField } from '@/ui/DateField';
import { InlineAlert } from '@/ui/InlineAlert';
import { Select } from '@/ui/Select';
import { toast } from '@/ui/Toast';

import { type Candidate, useCandidates, useTargetOrg } from './candidates';
import { type Changes } from './changes';
import { askToLetIn, letInChange, offerFor } from './doors';
import { CoversLine, type PlaceKind, type Reach, ReachSelect } from './Covers';
import { firstChoice, keptLine, mayGiveAsRole, mayGiveChoice, ROLE_OF_CHOICE, sharingIsKept, whyNot } from './gives';
import { HolderView, placeName } from './holders';
import { type Catalogue, CHOICE_ORDER, containerOf, middleOf, nameChange } from './model';
import { InviteOutsider, looksLikeAddress, outsiderReason } from './Outsiders';
import { tellable, TellThem, type TellState, tellThem, useDirectOff } from './Tell';

/** The id of the field that adds people, which Share gives the keyboard. */
const ADD_ID = 'access-add';

/** One access request takes at most eight things or people; larger selections keep the same entries in successive batches. */
const REQUEST_ITEMS = 8;

/** The words of a choice for a kind of thing: a node is interacted with, a resource used, a document edited. */
export function choiceWords(t: TFunction, choice: AccessChoice, kind: string): string {
  if (choice === 'full_view') return t('access.choice.view');
  if (choice === 'full_manage') return t('access.choice.manage');
  const middle = middleOf(kind);
  return t(`access.choice.${middle}`);
}

/** A sentence after “can” needs a verb, rather than the label “Can …” of a permission choice. */
export function accessVerb(t: TFunction, kind: string, line: 'view' | 'interact' | 'manage'): string {
  const scope = kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread' || kind === 'document' || kind === 'chat' ? kind : 'resource';
  return t(`access.roles.verb.${scope}.${line}`);
}

/** The short permission term precedes the existing explanation for this kind of thing. */
export function accessOption(t: TFunction, choice: AccessChoice, kind: string) {
  const line = choice === 'full_view' ? 'view' : choice === 'full_manage' ? 'manage' : 'interact';
  const term = line === 'interact' ? middleOf(kind) : line;
  return { ...choiceOption(t, `accessLevel.${term}`), description: hintOf(lineHintId(kind, line), t).what };
}

export interface AddHoldersProps {
  page: PermissionPage;
  target: AccessTarget;
  catalogue: Catalogue;
  changes: Changes;
  /** The choice the field begins on. */
  initial?: AccessChoice;
  /** What naming somebody in a choice includes, where that needs saying: a thread on a person's machine. */
  note?: (choice: AccessChoice) => string | null;
  /** Called once the people are named. */
  onNamed?: (candidates: readonly Candidate[], choice: AccessChoice) => void;
  /** Gives the field the keyboard when it opens, as Share does. */
  autoFocus?: boolean;
  /** Asks the people who can share here to let the reader: shown beside the field where the place keeps sharing to owners. */
  onRequest?: () => void;
  /** Share's one environment tick also applies to the general audience. */
  environmentChoice?: { checked: boolean; onChange: (checked: boolean) => void };
}

/** What went wrong with naming somebody as a role, in words: a role they hold already is said so, the rest is the server's own reason. */
function RoleFailure({ error, name }: { error: unknown; name: string }) {
  const { t } = useTranslation();
  if (error instanceof ApiError && error.is('conflict.exists')) return <p className="text-sm text-danger-text">{t('access.reach.holdsAlready', { name })}</p>;
  return <ErrorAlert error={error} />;
}

export function AddHolders({ page, target, catalogue, changes, initial = 'full_view', note, onNamed, autoFocus = false, onRequest, environmentChoice }: AddHoldersProps) {
  const { t } = useTranslation();
  const orgId = useTargetOrg(target);
  const place: PlaceKind | null = isPlaceKind(target.kind) ? target.kind : null;
  const [query, setQuery] = useState('');
  const [staged, setStaged] = useState<Candidate[]>([]);
  const [reach, setReach] = useState<Reach>('inside');
  const [wanted, setChoice] = useState<AccessChoice>(() => firstChoice(page, initial));
  const [until, setUntil] = useState('');
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [inviting, setInviting] = useState<string | null>(null);
  const [failed, setFailed] = useState<{ error: unknown; name: string } | null>(null);
  const [tell, setTell] = useState<TellState>({ on: true, line: '' });
  const [takeAway, setTakeAway] = useState(true);
  const directOff = useDirectOff(target);
  const actKey = useActKey();
  const [ownLetIn, setOwnLetIn] = useState(true);
  const letIn = environmentChoice?.checked ?? ownLetIn;
  const setLetIn = environmentChoice?.onChange ?? setOwnLetIn;
  const [waiting, setWaiting] = useState<string | null>(null);

  const inside = place !== null && reach === 'inside';
  const kept = sharingIsKept(page);
  // A place shared with what is in it gives a role, which is what the choice names there; anything else names the person in the rows.
  const mayGive = (entry: AccessChoice) => (inside ? mayGiveAsRole(page, entry) : mayGiveChoice(page, entry));

  // Share opens with the field that adds people in focus; the page it stands in arrives after the dialog does.
  useEffect(() => {
    if (autoFocus) document.getElementById(ADD_ID)?.focus();
  }, [autoFocus]);

  // What was chosen stays what the reader may give when the reach changes, or the page does: the lowest they may give takes its place.
  const choice = mayGive(wanted) ? wanted : (CHOICE_ORDER.find(mayGive) ?? wanted);

  const choices = CHOICE_ORDER.filter((entry) => catalogue.choice(entry, target.kind).length > 0);
  const off = outsiderReason(t, page, target);
  const defaultPlace = containerOf(page);
  const candidates = useCandidates(query, orgId, { kinds: ['user', 'group', 'role', 'members'], enabled: !kept && (query.trim() !== '' || optionsOpen), browsePlaces: optionsOpen, thing: target.name, outsiderOff: off, placeId: place !== null ? target.id : (defaultPlace === null || defaultPlace === 'system' ? undefined : defaultPlace.id) });
  const stagedKeys = new Set(staged.map((entry) => JSON.stringify(entry.input)));

  // An address nobody is registered with is invited, where the caller may bring somebody from outside. An invitation is to this one place, so a place shared with what is in it is shared on its own when one is chosen.
  const address = query.trim();
  const unknown = looksLikeAddress(address) && !candidates.loading && ![...candidates.byValue.values()].some((candidate) => candidate.user !== undefined);
  const groups: ComboboxGroup<string>[] = candidates.groups.map((group) => ({
    ...group,
    options: group.options.filter((option) => {
      const found = candidates.byValue.get(option.value);
      return found === undefined || !stagedKeys.has(JSON.stringify(found.input));
    }),
  }));
  if (unknown) {
    groups.push({
      id: 'invite',
      label: t('access.candidates.invite'),
      options: [
        {
          value: `invite:${address}`,
          label: t('access.candidates.inviteTo', { email: address }),
          text: t('access.candidates.inviteTo', { email: address }),
          icon: <Mail aria-hidden />,
          description: t('access.candidates.inviteAbout'),
          disabled: off ?? false,
        },
      ],
    });
  }

  const outsiders = staged.filter((entry) => entry.outside);
  const said = note?.(choice) ?? null;
  const why = whyNot(page, { t, name: target.name, kind: target.kind, choice: (entry) => choiceWords(t, entry, target.kind) }, { onPlace: place !== null });
  const expires = until === '' ? null : new Date(`${until}T23:59:59`).toISOString();

  // Giving a role is one request for each person: the ones that were given leave the list, the one that was refused stays with its reason.
  const giving = useMutation({
    mutationFn: async () => {
      const role: Role = ROLE_OF_CHOICE[choice];
      const given: Candidate[] = [];
      for (const entry of staged) {
        const subject = roleSubject(entry.input, target.id);
        if (subject === null) continue;
        try {
          await giveRole(target.id, { subject, role, ...(expires === null ? {} : { expires_at: expires }) });
          given.push(entry);
        } catch (error) {
          return { given, refused: { entry, error } };
        }
      }
      return { given, refused: null };
    },
    // The roles, the page, the people behind it and the members were asked again by the registry of acts as each role was given.
    onSuccess: ({ given, refused }) => {
      const names = given.map((entry) => entry.name).join(', ');
      if (given.length > 0) toast({ tone: 'success', title: t('access.reach.given', { names, count: given.length, role: t(`roles.${ROLE_OF_CHOICE[choice]}`), place: target.name }) });
      setStaged((before) => before.filter((entry) => !given.includes(entry)));
      setFailed(refused === null ? null : { error: refused.error, name: refused.entry.name });
      if (given.length > 0) {
        onNamed?.(given, choice);
        void tellThose(given);
      }
      if (refused === null) {
        setUntil('');
        setQuery('');
      }
    },
  });

  const working = changes.busy || giving.isPending;
  // Export travels with a choice: it is on as the form opens, and is the giver's to withhold when they may export the thing themself.
  const exportRows = page.rows.filter((row) => row.permission.endsWith('.export'));
  const mayExport = exportRows.some((row) => row.can_give);

  // A thread's files, terminal, browsers and displays are its environment's: sharing the thread offers to let the people named look into it, and with Can manage to change its files and open a shell.
  const offer = target.kind === 'thread' ? offerFor(page, 'full_view') : null;

  /** Lets the people named into the environment: named on its page where the person at this page may give there, and a request to its owner where not. */
  const lettingIn = async (people: readonly Candidate[]) => {
    if (offer === null || !letIn) return;
    const holders = people.map((entry) => entry.input);
    const environment = { kind: 'environment' as const, id: offer.environment.id, name: offer.environment.name };
    try {
      if (offer.mayView || offer.giveManage.length > 0) {
        const envPage = await readPage(environment);
        await changePage(environment, letInChange(envPage.revision, holders, offer, expires));
      }
      const ask = askToLetIn(offer, holders, t('access.doors.reason', { names: people.map((entry) => entry.name).join(', '), thread: target.name, name: offer.environment.name }), target.id);
      if (ask !== null) {
        for (let offset = 0; offset < ask.items.length; offset += REQUEST_ITEMS) {
          const batch = { ...ask, items: ask.items.slice(offset, offset + REQUEST_ITEMS) };
          await requestAccess(batch, actKey(batch));
        }
        setWaiting(t('access.doors.waiting', { names: people.map((entry) => entry.name).join(', '), owner: offer.environment.owner?.display_name ?? '', name: offer.environment.name }));
      }
    } catch {
      toast({ tone: 'danger', title: t('access.doors.failed', { names: people.map((entry) => entry.name).join(', '), name: offer.environment.name }) });
    }
  };

  /** Tells the people named, after they were named: a message each, in the chat the two have. */
  const tellThose = async (people: readonly Candidate[]) => {
    const told = directOff || !tell.on ? [] : tellable(people);
    if (told.length === 0 || orgId === null) return;
    const refused = await tellThem([target], orgId, told, tell.line, (name) => t('access.tell.wording', { name }));
    if (refused.length > 0) toast({ tone: 'danger', title: t('access.tell.failed', { names: refused.join(', ') }) });
  };
  const add = async () => {
    if (staged.length === 0) return;
    setFailed(null);
    if (inside) {
      giving.mutate();
      return;
    }
    const names = staged.map((entry) => entry.name).join(', ');
    const made = await changes.run({
      label: t('access.add.adding', { names }),
      make: (current) => nameChange(current, choice, staged.map((entry) => entry.input), { expires_at: expires, ...(exportRows.length > 0 && mayExport && !takeAway ? { export: false } : {}) }),
    });
    if (!made) return;
    onNamed?.(staged, choice);
    void tellThose(staged);
    void lettingIn(staged);
    setStaged([]);
    setUntil('');
    setQuery('');
  };

  if (kept) {
    // The place keeps sharing to a thing's owners: the field is there and off, and says who can.
    return (
      <div className="flex flex-col gap-2" data-tutorial-target="holder-picker">
        <div className="flex flex-wrap items-start gap-2">
          <Combobox label={t('access.add.label')} hideLabel placeholder={t('access.add.placeholder')} query="" onQueryChange={() => undefined} onSelect={() => undefined} groups={[]} disabled fieldClassName="min-w-[12rem] flex-[3_1_14rem]" />
          {onRequest !== undefined && (
            <Button onClick={onRequest}>{t('access.gives.request')}</Button>
          )}
        </div>
        <p className="text-sm text-text-muted">{keptLine(t, page, place !== null ? target.name : placeName(t, containerOf(page)))}</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2" data-tutorial-target="holder-picker">
      <div className="flex flex-wrap items-start gap-2">
        <Combobox
          label={t('access.add.label')}
          hideLabel
          placeholder={t('access.add.placeholder')}
          query={query}
          onQueryChange={setQuery}
          minChars={1}
          loading={candidates.loading}
          empty={t('access.candidates.none')}
          groups={groups}
          fieldClassName="min-w-[12rem] flex-[3_1_14rem]"
          onSelect={(option) => {
            if (option.value.startsWith('invite:')) {
              if (inside) setReach('alone');
              setInviting(option.value.slice('invite:'.length));
              setQuery('');
              return;
            }
            const found = candidates.byValue.get(option.value);
            if (found === undefined) return;
            setStaged((before) => [...before, found]);
            setQuery('');
          }}
          {...(autoFocus ? { id: ADD_ID } : {})}
        />
        <div className="flex flex-[1_1_9rem] items-center gap-1">
          <Select<AccessChoice>
            label={t('access.add.choice')}
            hideLabel
            value={choice}
            onValueChange={setChoice}
            options={choices.map((entry) => ({
              value: entry,
              ...accessOption(t, entry, target.kind),
              disabled: !mayGive(entry),
              ...(mayGive(entry) ? {} : { description: t('access.gives.notYours') }),
            }))}
            fieldClassName="min-w-0 flex-1"
          />
        </div>
        <Button variant="primary" icon={<Plus aria-hidden />} pending={working} blocked={staged.length === 0 || !mayGive(choice)} onClick={() => void add()}>
          {t('access.add.button')}
        </Button>
        <Button variant="ghost" aria-expanded={optionsOpen} onClick={() => setOptionsOpen(!optionsOpen)}>{t('access.roster.options')}</Button>
      </div>

      {optionsOpen && <div className="flex flex-wrap items-start gap-3 rounded-md border border-border p-3">
        {candidates.placePicker}
        {place !== null && <ReachSelect place={target.name} value={reach} onValueChange={setReach} className="flex-[1_1_12rem]" />}
        <DateField label={t('access.add.until')} value={until} onValueChange={setUntil} aria-label={t('access.add.untilLabel')} fieldClassName="w-[12rem]" />
        {!inside && exportRows.length > 0 && (mayExport
          ? <Checkbox label={t('access.export.label')} checked={takeAway} onCheckedChange={setTakeAway} />
          : <p className="text-sm text-text-muted">{t('access.export.cannot')}</p>)}
      </div>}

      {why !== null && <p className="text-sm text-text-muted">{why}</p>}
      {place !== null && <CoversLine page={page} kind={place} name={target.name} reach={reach} />}

      {staged.length > 0 && (
        <ul className="flex flex-wrap items-center gap-2" aria-label={t('access.add.staged')}>
          {staged.map((entry) => (
            <li key={JSON.stringify(entry.input)}>
              <HolderView
                holder={entry.holder}
                target={target}
                outside={entry.outside}
                pending={working}
                remove={{
                  label: t('access.add.unstage', { name: entry.name }),
                  disabled: working,
                  onRemove: () => setStaged((before) => before.filter((other) => other !== entry)),
                }}
              />
            </li>
          ))}
        </ul>
      )}

      {staged.length > 0 && <TellThem target={target} staged={staged} state={tell} onChange={setTell} />}
      {offer !== null && staged.length > 0 && environmentChoice === undefined && (
        <Checkbox
          label={offer.mayView ? t('access.doors.letIn', { name: offer.environment.name }) : t('access.doors.askFor', { owner: offer.environment.owner?.display_name ?? '' })}
          checked={letIn}
          onCheckedChange={setLetIn}
        />
      )}
      {waiting !== null && <InlineAlert tone="info">{waiting}</InlineAlert>}

      {failed !== null && <RoleFailure error={failed.error} name={failed.name} />}
      {outsiders.length > 0 && <p className="text-sm text-text-muted">{t('access.add.outsideNote', { names: outsiders.map((entry) => entry.name).join(', '), place: target.name })}</p>}
      {said !== null && staged.length > 0 && <InlineAlert tone="warning">{said}</InlineAlert>}
      {inviting !== null && <InviteOutsider target={target} choice={choice} catalogue={catalogue} email={inviting} open onClose={() => setInviting(null)} />}
    </div>
  );
}
