/** Share: owners, a compact add field and the people, groups and roles with their level and source.
 *
 * It is the page people use every day, and the same page as the full form with fewer words. A holder
 * stands in the highest of the three lines it holds completely (a role is also everybody above it);
 * what does not make one of the three is listed as Custom, with what it holds in the catalogue's
 * words. Whatever the page follows from a place is said on the chip; editing it freezes only the
 * touched rows with their other holders intact. Every act waits for the server's answer and shows as
 * pending meanwhile; a page that moved on meanwhile is shown as it is now, with what was meant for it.
 */
import { useQuery } from '@tanstack/react-query';
import { Copy, Grid3x3, ShieldAlert } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessTarget, isNodeKind, peopleQuery, readPage } from '@/api/access';
import { meQuery } from '@/api/auth';
import { environmentQuery } from '@/api/environments';
import type { AccessChoice, MissingAccess, NodeOrSystem, PermissionPage } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { denial } from '@/lib/permissions';
import { actionName } from '@/ui/actions';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { HolderChip, type PeopleSource } from '@/ui/HolderChip';
import { InlineAlert } from '@/ui/InlineAlert';
import { Dialog } from '@/ui/Dialog';
import { copyText } from '@/ui/clipboard';

import { AccessRequest } from './AccessRequest';
import { AccessRoster } from './AccessRoster';
import { AddHolders, choiceWords } from './AddHolders';
import type { Asked } from './ChangePreview';
import type { Changes, PageSource } from './changes';
import { givesNothing } from './gives';
import { GoesWith } from './GoesWith';
import { letThreadViewersIn, offerFor } from './doors';
import { GeneralAccessControl, generalAccessWords } from './GeneralAccess';
import { holderName, placeName } from './holders';
import {
  type Catalogue,
  containerOf,
  followedPlace,
  generalAccessChange,
  generalAccessOf,
  type GeneralAccessPreset,
  holderLevelChange,
  type LineHolder,
  type LineId,
  linesOf,
  removeHolderChange,
  wholeChange,
} from './model';
import { Owners } from './Owners';
import { WaitingRequests } from './RequestCard';
import { EnvironmentReachLine, privateWarning, ReachesLine } from './Reaches';
import { linkOf } from './target';

/** What a page that moved on meanwhile says: that it is shown as it is now, what was meant for it, and what can be done. */
export function StaleNotice({ changes }: { changes: Changes }) {
  const { t } = useTranslation();
  if (changes.stale === null) return null;
  return (
    <InlineAlert
      tone="warning"
      title={t('access.stale.title')}
      action={
        <>
          <Button size="sm" variant="primary" pending={changes.busy} onClick={() => void changes.again()}>
            {t('access.stale.again')}
          </Button>
          <Button size="sm" variant="ghost" disabled={changes.busy} onClick={changes.forget}>
            {t('access.stale.leave')}
          </Button>
        </>
      }
    >
      <p>{t('access.stale.body')}</p>
      <p className="text-text-muted">{t('access.stale.notMade', { what: changes.stale.label })}</p>
    </InlineAlert>
  );
}

/** What the last change was refused with, beside the page; a page that moved on is said by `StaleNotice`. */
export function ChangeFailed({ changes }: { changes: Changes }) {
  if (changes.error === null || changes.error === undefined) return null;
  return <ErrorAlert error={changes.error} />;
}

interface ShortFormProps {
  target: AccessTarget;
  page: PermissionPage;
  catalogue: Catalogue;
  source: PageSource;
  changes: Changes;
  ask: (asked: Asked) => void;
  onFull: () => void;
  /** Legacy callers may supply it; the unavailable View as action is not offered. */
  onViewAs?: () => void;
  /** Gives the add field the keyboard, as Share does. */
  focusAdd?: boolean;
  /** Opens the page of another thing, such as the environment a thread's files are those of. */
  onOpen?: ((target: AccessTarget) => void) | undefined;
}

export function ShortForm({ target, page, catalogue, changes, ask, onFull, focusAdd = false, onOpen }: ShortFormProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const lines = linesOf(page, catalogue, target.kind);
  // What the page follows is the place it lies in, whichever place's defaults the rows end up drawing from.
  const place = containerOf(page) ?? followedPlace(page.rows);
  const placeText = placeName(t, place);
  const connected = page.reaches?.environment.placement === 'connected';
  // An environment on a person's machine has the same rows, and naming somebody to manage it says what that includes.
  const environment = useQuery({ ...environmentQuery(target.id), enabled: target.kind === 'environment' });
  const machineOwner = target.kind === 'environment' && environment.data?.placement === 'connected' ? environment.data.owner.display_name : null;
  const me = useQuery(meQuery).data;
  // Whoever manages the page changes it as they like; whoever shares without managing it adds people up to what they hold and takes back whom they added, and reads the rest.
  const mayChange = page.owners_writable || page.rows.some((row) => row.writable);
  const readOnlyReason = denial(t, 'access.manage', target.name, page.owner);
  const sharing = !givesNothing(page) || !page.gives.holders_share;
  const [requesting, setRequesting] = useState(false);
  const [letIn, setLetIn] = useState(true);
  const [environmentError, setEnvironmentError] = useState<unknown>(null);
  const [lettingIn, setLettingIn] = useState(false);
  const audienceId = target.kind === 'org' ? target.id : place !== null && place !== 'system' ? place.id : target.orgId ?? '';
  const audiencePlace = target.kind === 'org' ? target.name : placeText;
  const general = generalAccessOf(page, catalogue, target.kind, audienceId);
  const environmentOffer = target.kind === 'thread' ? offerFor(page, 'full_view') : null;
  const generalWritable = page.rows.every((row) => row.permission === 'access.own' || row.writable);

  const letReadersIn = async () => {
    if (!letIn || environmentOffer === null) return;
    setLettingIn(true);
    setEnvironmentError(null);
    try {
      const confirmed = await readPage(target);
      if (!await letThreadViewersIn(confirmed)) setEnvironmentError(new Error(t('access.general.cannotLetIn', { name: environmentOffer.environment.name })));
    } catch (error) {
      setEnvironmentError(error);
    } finally {
      setLettingIn(false);
    }
  };

  const take = (holder: LineHolder, line: LineId) => {
    const name = holder.holder.kind === 'user' ? holder.holder.user.display_name : holderName(t, holder.holder, target.name);
    ask({
      title: t('access.line.takeOutTitle', { name }),
      body: t('access.line.takeOutBody', { name, line: line === 'custom' ? t('access.lines.custom') : choiceWords(t, lineChoice(line), target.kind) }),
      confirmLabel: t('access.line.takeOutConfirm', { name }),
      danger: true,
      // Whoever does not manage the page may take back only what they gave, and is not told whom it reaches: that is asked of the page's managers.
      preview: mayChange,
      label: t('access.line.takingOut', { name }),
      make: (current) => removeHolderChange(current, holder),
    });
  };


  const setGeneral = (preset: GeneralAccessPreset) => {
    if (preset === 'defaults') { followAgain(); return; }
    const choice = generalAccessWords(t, catalogue, target.kind, audiencePlace, preset);
    ask({
      title: t('access.general.title', { choice }),
      body: t('access.general.about'),
      confirmLabel: t('access.general.confirm'),
      danger: true,
      label: t('access.general.changing'),
      make: (current) => generalAccessChange(catalogue, target.kind, audienceId, preset, current),
      after: () => { void letReadersIn(); },
    });
  };

  const changeLevel = (holder: LineHolder, choice: AccessChoice) => {
    const name = holder.holder.kind === 'user' ? holder.holder.user.display_name : holderName(t, holder.holder, target.name);
    ask({ title: t('access.general.holderTitle', { name, choice: choiceWords(t, choice, target.kind) }),
      body: t('access.general.holderBody'), confirmLabel: t('access.general.changeHolder', { name }), danger: true,
      label: t('access.general.changingHolder', { name }), make: (current) => holderLevelChange(current, catalogue, target.kind, holder, choice) });
  };

  const makePrivate = () => {
    const warning = privateWarning(t, page);
    ask({
      title: t('access.state.privateTitle', { name: target.name }),
      body: (
        <>
          <p>{t('access.state.privateBody', { place: placeText })}</p>
          {warning !== null && <p className="mt-2">{warning}</p>}
        </>
      ),
      confirmLabel: actionName('makePrivate', target.name),
      danger: true,
      label: t('access.state.makingPrivate', { name: target.name }),
      make: (current) => generalAccessChange(catalogue, target.kind, audienceId, 'private', current),
    });
  };
  const followAgain = () =>
    ask({
      title: t('access.state.followTitle', { place: placeText }),
      body: t('access.state.followBody', { place: placeText }),
      confirmLabel: t('craftAccess.inherit', { place: placeText }),
      danger: true,
      label: t('access.state.following', { place: placeText }),
      make: (current) => wholeChange(current, 'reset'),
    });


  const url = linkOf(target);
  const copy = async () => {
    if (url !== null) await copyText(url, t('common.linkCopied'));
  };

  const adminPlaces = page.admins.of.map((entry: NodeOrSystem) => placeName(t, entry));
  const adminNames = new Intl.ListFormat(locale, { type: 'conjunction' }).format(adminPlaces);
  const admins: PeopleSource | undefined = isNodeKind(target.kind) ? (peopleQuery(target, 'admins') as unknown as PeopleSource) : undefined;

  // What the reader asks for when the place keeps sharing to owners: to be one who may share here, which is managing the page.
  const asking: MissingAccess[] = [
    {
      object: { id: target.id, kind: target.kind, name: target.name, owner: page.owner },
      permissions: ['access.manage'],
      options: null,
      ask: page.owners.flatMap((entry) => (entry.holder.kind === 'user' ? [entry.holder.user] : [])),
      ask_more: 0,
      override: false,
      closed: null,
    },
  ];
  const interactNote = (choice: AccessChoice): string | null => {
    if (machineOwner !== null) return choice === 'full_manage' ? t('access.connected.environment', { owner: machineOwner, name: target.name }) : null;
    if (!connected || page.reaches === null) return null;
    const owner = page.reaches.environment.owner.display_name;
    if (choice === 'full_interact') return t('access.connected.interact', { owner });
    if (choice === 'full_manage') return t('access.connected.manage', { owner });
    return null;
  };

  return (
    <div className="flex flex-col gap-6">
      <StaleNotice changes={changes} />
      <ChangeFailed changes={changes} />
      <GeneralAccessControl catalogue={catalogue} kind={target.kind} place={audiencePlace} rulesPlace={placeText} value={general} onChange={setGeneral}
        disabled={!generalWritable || changes.busy || lettingIn} hasAudience={audienceId !== ''} canGive={page.gives.choices} onReset={followAgain}
        {...(environmentOffer === null ? {} : { environment: { checked: letIn, onChange: setLetIn, disabled: changes.busy || lettingIn } })} />
      {environmentError !== null && <ErrorAlert error={environmentError} />}

      {changes.busy && changes.doing !== null && <p role="status" className="text-sm text-text-muted">{changes.doing}…</p>}

      {connected && page.reaches !== null && (
        <InlineAlert tone="info">{t('access.connected.lead', { owner: page.reaches.environment.owner.display_name, name: page.reaches.environment.name, place: placeText })}</InlineAlert>
      )}

      {mayChange && isNodeKind(target.kind) && <WaitingRequests nodeId={target.id} />}

      <Owners page={page} target={target} changes={changes} />

      {sharing ? (
        <>
          <AddHolders page={page} target={target} catalogue={catalogue} changes={changes} note={interactNote} onRequest={() => setRequesting(true)} {...(focusAdd ? { autoFocus: true } : {})} {...(environmentOffer === null ? {} : { environmentChoice: { checked: letIn, onChange: setLetIn } })} />
          {!mayChange && !givesNothing(page) && <p className="text-sm text-text-muted">{t('access.sharerOnly')}</p>}
        </>
      ) : (
        <InlineAlert tone="info">{t('access.readOnly', { reason: readOnlyReason })}</InlineAlert>
      )}

      {target.kind === 'environment' && <EnvironmentReachLine target={target} />}

      <AccessRoster lines={lines} target={target} page={page} catalogue={catalogue} changes={changes} meId={me?.id} onTake={take} onLevel={changeLevel} onOpen={onOpen} />
      <GoesWith page={page} target={target} />

      {(page.admins.count > 0 || page.reaches !== null) && (
        <section aria-label={t('access.reaches.title')} className="flex flex-col gap-2">
          <h3 className="text-sm font-medium text-text">{t('access.reaches.title')}</h3>
          {page.admins.count > 0 && (
            <div className="flex flex-wrap items-center gap-2">
              <HolderChip
                name={t('access.reaches.admins', { places: adminNames })}
                count={page.admins.count}
                icon={<ShieldAlert aria-hidden />}
                people={admins}
                inherited
              />
              <span className="text-sm text-text-muted">{t('access.roster.adminLevel')}</span>
            </div>
          )}
          <ReachesLine page={page} onOpen={onOpen} />
        </section>
      )}

      <div className="sticky bottom-0 z-(--layer-sticky) -mx-6 -mb-2 flex flex-wrap items-center justify-end gap-2 border-t border-border bg-surface-raised px-6 py-3">
        <Button icon={<Grid3x3 aria-hidden />} onClick={onFull}>
          {t('access.footer.byArea')}
        </Button>
        <Button
          variant="secondary"
          action={page.state === 'restricted' ? 'inheritAccess' : 'makePrivate'}
          onClick={page.state === 'restricted' ? followAgain : makePrivate}
          blocked={!mayChange}
        >
          {page.state === 'restricted' ? t('craftAccess.inherit', { place: placeText }) : actionName('makePrivate')}
        </Button>
        {url !== null && (
          <Button icon={<Copy aria-hidden />} onClick={() => void copy()}>
            {t('access.footer.copyLink')}
          </Button>
        )}
      </div>

      {requesting && (
        <Dialog open onOpenChange={(next) => !next && setRequesting(false)} size="sm" title={t('access.gives.requestTitle', { name: target.name })} description={t('access.gives.requestLead')}>
          <AccessRequest missing={asking} objectId={target.id} fixed />
        </Dialog>
      )}
    </div>
  );
}

/** The choice a line stands for. */
export function lineChoice(line: LineId): AccessChoice {
  return line === 'manage' ? 'full_manage' : line === 'interact' ? 'full_interact' : 'full_view';
}


