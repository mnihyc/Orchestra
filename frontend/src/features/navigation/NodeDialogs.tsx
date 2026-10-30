/** The dialogs that make a place or change one: a new org, department, project and topic, rename, archive, delete, move, what was deleted, and joining by a link.
 *
 * A folder is made by the same dialog that makes a project, by whoever opens topics where it is to lie.
 * The colour of a place's icon is given here too, and the dialogs of a document that stands in the tree
 * (rename, move, delete) are drawn by the same host.
 *
 * Every menu that acts on a place opens its dialog through `openNodeDialog`, and the one host of the
 * frame draws it, so a row of the left bar, the "⋯" of a screen and a button in a toolbar all lead to
 * the same dialog. A dialog asks for what the act needs, says what it will do before it does it, and
 * shows what the server refused with, in place, with what is missing and whom to ask. Each act that
 * creates carries one key for all its attempts, so that a press made twice makes one thing.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminSpec } from '@/ui/AdminAct';
import { choiceOption } from '@/i18n/choices';
import { useStore } from 'zustand';
import { create } from 'zustand';

import {
  applyArchivePlan,
  createDepartment,
  createFolder,
  createOrg,
  createProject,
  createTopic,
  deleteNode,
  nodeKey,
  nodeQuery,
  previewDelete,
  previewArchive,
  restoreNode,
  updateNode,
} from '@/api/nodes';
import { metaKey, metaQuery, meQuery } from '@/api/auth';
import { ApiError, client } from '@/api/client';
import { useMakeDocument } from '@/features/documents/useMakeDocument';
import { ColourDialog } from '@/features/navigation/Colour';
import { depthWords } from '@/features/navigation/depth';
import { useNamedPlace } from '@/features/navigation/namedPlace';
import { ArchiveConsequences } from '@/features/project/ReviewConsequences';
import { DeleteDocumentDialog, MoveDocumentDialog, RenameDocumentDialog } from '@/features/navigation/DocDialogs';
import { RecentlyDeleted } from '@/features/navigation/RecentlyDeleted';
import { type DocPlace, type PlaceRef, placePath, type SubtreeCounts, subtreeCounts } from '@/features/navigation/places';
import type { Node, NodeDeletePlan, Role } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useActKey, useForm } from '@/lib/forms';
import { formatExact } from '@/lib/time';
import { pathTo, useTreeStore } from '@/realtime/stores/tree';
import { selectOrg } from '@/shell/org';
import { useCurrentHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Textarea } from '@/ui/Textarea';
import { useErrorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';
import { MoveDialog } from '@/features/org/Move';
import { NewThreadDialog } from '@/features/start/BindingDialog';

/** What can be asked for: each is a dialog the host draws. */
export type NodeDialog =
  | { type: 'new-org' }
  | { type: 'join' }
  | { type: 'new-department'; parent: PlaceRef }
  | { type: 'new-project'; parent: PlaceRef }
  | { type: 'new-folder'; parent: PlaceRef }
  | { type: 'new-topic'; parent: PlaceRef }
  | { type: 'new-thread'; parent: PlaceRef }
  | { type: 'new-document'; parent: PlaceRef }
  | { type: 'rename'; node: PlaceRef }
  | { type: 'archive'; node: PlaceRef }
  | { type: 'delete'; node: PlaceRef }
  | { type: 'move'; node: PlaceRef; to?: string }
  | { type: 'deleted'; node: PlaceRef }
  | { type: 'colour'; node: PlaceRef }
  | { type: 'rename-document'; doc: DocPlace }
  | { type: 'move-document'; doc: DocPlace }
  | { type: 'delete-document'; doc: DocPlace }
  | { type: 'error'; title: string; error: unknown };

/** How long a dialog that was closed stays drawn, for its exit: the slow step of the motion tokens. */
const EXIT_MS = 200;

interface DialogState {
  current: NodeDialog | null;
  open: boolean;
  show: (dialog: NodeDialog) => void;
  hide: () => void;
}

const useDialogState = create<DialogState>((set, get) => ({
  current: null,
  open: false,
  show: (current) => set({ current, open: true }),
  hide: () => {
    set({ open: false });
    // The dialog leaves with its exit, and is forgotten once it has.
    setTimeout(() => {
      if (!get().open) set({ current: null });
    }, EXIT_MS);
  },
}));

/** Opens one of the dialogs. */
export function openNodeDialog(dialog: NodeDialog): void {
  useDialogState.getState().show(dialog);
}

/** Draws the dialog that was asked for. Stands once in the frame of the application. */
export function NodeDialogHost() {
  const current = useDialogState((state) => state.current);
  const open = useDialogState((state) => state.open);
  const hide = useDialogState((state) => state.hide);
  if (current === null) return null;
  const shared = { open, onClose: hide };
  switch (current.type) {
    case 'new-org':
      return <MakeDialog kind="org" {...shared} />;
    case 'new-department':
      return <MakeDialog kind="department" parent={current.parent} {...shared} />;
    case 'new-project':
      return <MakeDialog kind="project" parent={current.parent} {...shared} />;
    case 'new-folder':
      return <MakeDialog kind="folder" parent={current.parent} {...shared} />;
    case 'new-topic':
      return <MakeDialog kind="topic" parent={current.parent} {...shared} />;
    case 'join':
      return <JoinDialog {...shared} />;
    case 'new-thread':
      return <NewThreadDialog parent={current.parent} open={shared.open} onClose={shared.onClose} />;
    case 'new-document':
      return <NewDocumentDialog parent={current.parent} {...shared} />;
    case 'rename':
      return <RenameDialog place={current.node} {...shared} />;
    case 'archive':
      return <ArchiveDialog place={current.node} {...shared} />;
    case 'delete':
      return <DeleteDialog place={current.node} {...shared} />;
    case 'move':
      return <MoveDialog place={current.node} {...(current.to === undefined ? {} : { to: current.to })} {...shared} />;
    case 'deleted':
      return <RecentlyDeleted place={current.node} {...shared} />;
    case 'colour':
      return <ColourDialog place={current.node} {...shared} />;
    case 'rename-document':
      return <RenameDocumentDialog doc={current.doc} {...shared} />;
    case 'move-document':
      return <MoveDocumentDialog doc={current.doc} {...shared} />;
    case 'delete-document':
      return <DeleteDocumentDialog doc={current.doc} {...shared} />;
    case 'error':
      return <Dialog open={open} onOpenChange={(next) => { if (!next) hide(); }} title={current.title}><ErrorAlert error={current.error} /></Dialog>;
  }
}

interface DialogProps {
  open: boolean;
  onClose: () => void;
}

// ---------------------------------------------------------------- making

type MakeKind = 'org' | 'department' | 'project' | 'folder' | 'topic';

/** The fewest characters of a short name: a root org's is the address of everything in it. */
const SLUG_MIN = { org: 3, department: 2 } as const;
const SLUG_MAX = 32;
const SLUG_SHAPE = /^[a-z0-9][a-z0-9_-]*$/;

/** A short name from a name: lower case, letters and digits, with a hyphen where anything else stood. */
export function slugOf(name: string): string {
  return name
    .normalize('NFKD')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, SLUG_MAX)
    .replace(/-+$/g, '');
}

/** How often an open dialog for a team org asks again who may make one, so that a change of the system's policy reaches it. */

/**
 * Whether the person may make a team org now, read afresh while the dialog for one is open: `null`
 * while they may (or it is not known yet), else the reason, as the system's `orgs.created_by` gives it.
 * Under "invited", `usesInvitation` says that making one uses the invitation a system admin gave.
 */
function useOrgPolicy(open: boolean, wanted: boolean) {
  const { t } = useTranslation();
  const meta = useQuery({ ...metaQuery, enabled: wanted && open }).data;
  const admin = useQuery({ ...meQuery, enabled: wanted }).data?.is_system_admin === true;
  if (!wanted || meta === undefined) return { refused: null, usesInvitation: false };
  if (!meta.may_create_org) {
    return { refused: meta.org_creation === 'admins' ? t('shell.rail.orgCreation.admins') : t('shell.rail.orgCreation.invited'), usesInvitation: false };
  }
  return { refused: null, usesInvitation: meta.org_creation === 'invited' && !admin };
}

function MakeDialog({ kind, parent, open, onClose }: DialogProps & { kind: MakeKind; parent?: PlaceRef }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const userId = useStore(client.session, (state) => state.userId);
  const actKey = useActKey();
  const [priv, setPriv] = useState(false);
  const slugEdited = useRef(false);
  const hasSlug = kind === 'org' || kind === 'department';
  const parentNode = useQuery({ ...nodeQuery(parent?.id ?? ''), enabled: kind === 'department' && parent !== undefined });
  const parentAddress = parentNode.data?.kind === 'org' ? parentNode.data.address : null;
  // A team org is made only by whom the system lets: the draft stays while the policy says no, and
  // the dialog follows a change of it while it is open.
  const policy = useOrgPolicy(open, kind === 'org');

  const form = useForm(
    { name: '', slug: '', description: '' },
    {
      name: (value) => (kind !== 'topic' && value.trim() === '' ? t('places.rules.nameMissing') : null),
      slug: (value) => {
        if (!hasSlug) return null;
        const fewest = SLUG_MIN[kind === 'org' ? 'org' : 'department'];
        if (value === '') return t('places.rules.slugMissing');
        return SLUG_SHAPE.test(value) && value.length >= fewest && value.length <= SLUG_MAX
          ? null
          : t('places.rules.slugShape', { min: fewest, max: SLUG_MAX });
      },
    },
  );

  const make = useMutation({
    mutationFn: async (values: { name: string; slug: string; description: string }) => {
      const name = values.name.trim();
      const description = values.description.trim();
      const access = priv ? ({ access: 'private' } as const) : {};
      const key = actKey({ kind, parent: parent?.id, ...values, priv });
      switch (kind) {
        case 'org':
          return createOrg({ name, slug: values.slug }, key);
        case 'department':
          return createDepartment(parent?.id ?? '', { name, slug: values.slug, ...(description === '' ? {} : { description }), ...access }, key);
        case 'project':
          return createProject(parent?.id ?? '', { name, ...(description === '' ? {} : { description }), ...access }, key);
        case 'folder':
          return createFolder(parent?.id ?? '', { name, ...(description === '' ? {} : { description }), ...access }, key);
        case 'topic':
          return createTopic(
            parent?.id ?? '',
            { ...(name === '' ? {} : { name }), ...(description === '' ? {} : { description }), ...access },
            key,
          );
      }
    },
    // What the new place is a row of is asked again by the registry of acts before this runs.
    onSuccess: (made) => {
      onClose();
      if (made.kind === 'org') {
        if (made.parent_id === null) selectOrg(userId, made.id);
        void navigate({ to: '/org/$slug', params: { slug: made.address } });
      } else if (made.kind === 'project') {
        void navigate({ to: '/project/$projectId', params: { projectId: made.id } });
      } else {
        void navigate({ to: '/topic/$topicId', params: { topicId: made.id } });
      }
    },
    onError: (error) => {
      // A refusal by the policy is told with the server's reason, and what the system says of who may
      // make an org is read again; the draft stays.
      if (error instanceof ApiError && error.is('org.creation_disabled')) void queryClient.invalidateQueries({ queryKey: metaKey });
      void form.refuse(error);
    },
  });

  const name = form.field('name');
  const slug = form.field('slug');
  const taken = make.error instanceof ApiError && make.error.is('conflict.exists') && hasSlug;
  // The org of this place that has the short name already, where the server names it, and a short name that is free.
  const clash = taken && make.error instanceof ApiError ? make.error.extra('conflict.exists') : null;
  const takenWords =
    clash?.existing === null || clash?.existing === undefined
      ? t('places.rules.slugTaken')
      : t(clash.existing.deleted ? 'places.rules.slugTakenDeleted' : 'places.rules.slugTakenBy', { name: clash.existing.name, slug: slug.value });
  const placed = make.error instanceof ApiError && make.error.faults().some((fault) => fault.path in form.values);
  const where = parent?.name ?? '';

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t(`places.make.${kind}.title`, { parent: where })}
      description={t(`places.make.${kind}.lead`, { parent: where })}
      dismissible={!make.isPending}
      footer={
        <>
          <Button disabled={make.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button type="submit" form="make-place" variant="primary" pending={make.isPending} disabled={policy.refused !== null}>
            {t(`places.make.${kind}.submit`)}
          </Button>
        </>
      }
    >
      <form
        id="make-place"
        noValidate
        className="flex flex-col gap-4 py-2"
        onSubmit={form.submit((values) => {
          if (!make.isPending && policy.refused === null) make.mutate(values);
        })}
      >
        {policy.refused !== null && (
          <InlineAlert
            tone="warning"
            action={
              <Button size="sm" onClick={() => openNodeDialog({ type: 'join' })}>
                {t('shell.rail.join')}
              </Button>
            }
          >
            {policy.refused}
          </InlineAlert>
        )}
        {policy.usesInvitation && <p className="text-sm text-text-muted">{t('places.make.org.usesInvitation')}</p>}
        <Input
          label={t('places.fields.name')}
          optional={kind === 'topic'}
          hint={kind === 'topic' ? t('places.fields.topicNameHint') : undefined}
          autoComplete="off"
          {...name}
          onChange={(event) => {
            form.set('name', event.target.value);
            if (hasSlug && !slugEdited.current) form.set('slug', slugOf(event.target.value));
          }}
        />
        {hasSlug && (
          <Input
            label={t('places.fields.slug')}
            hint={
              kind === 'department' && parentAddress !== null
                ? t('places.fields.slugAddress', { address: `${parentAddress}.${slug.value === '' ? '…' : slug.value}` })
                : t('places.fields.slugHint')
            }
            mono
            autoComplete="off"
            autoCapitalize="none"
            spellCheck={false}
            {...slug}
            error={taken ? takenWords : slug.error}
            onChange={(event) => {
              slugEdited.current = event.target.value !== '';
              form.set('slug', event.target.value.toLowerCase());
            }}
          />
        )}
        {clash?.suggestion !== null && clash?.suggestion !== undefined && (
          <div>
            <Button
              size="sm"
              onClick={() => {
                slugEdited.current = true;
                form.set('slug', clash.suggestion ?? '');
                make.reset();
              }}
            >
              {t('places.rules.slugOffer', { slug: clash.suggestion })}
            </Button>
          </div>
        )}
        {kind !== 'org' && (
          <Textarea label={t('places.fields.description')} optional {...form.field('description')} />
        )}
        {kind !== 'org' && <Checkbox label={t('places.fields.private')} hint={t('places.fields.privateHint')} checked={priv} onCheckedChange={setPriv} />}
        {make.error !== null && !taken && !placed && (
          <ErrorAlert error={make.error} message={make.error instanceof ApiError ? depthWords(make.error, t) : undefined} />
        )}
      </form>
    </Dialog>
  );
}

/** The menu's New document uses the same immediate creation as the list; only a refused creation opens recovery. */
export function NewDocumentDialog({ parent, open, onClose }: DialogProps & { parent: PlaceRef }) {
  const { t } = useTranslation();
  const make = useMakeDocument({ id: parent.id });
  const requested = useRef(false);
  const create = () => make.mutate({}, { onSuccess: onClose });
  useEffect(() => {
    if (!open || requested.current) return;
    requested.current = true;
    make.mutate({}, { onSuccess: onClose });
  }, [open, make, onClose]);
  if (make.error === null) return null;
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('places.make.document.title', { parent: parent.name })}
      dismissible={!make.isPending}
    >
      <ErrorState compact error={make.error} retrying={make.isPending} onRetry={create} />
    </Dialog>
  );
}

// ---------------------------------------------------------------- joining

/** The secret of an invitation from what was pasted: the whole link, the end of it, or the secret itself. */
export function tokenOf(pasted: string): string {
  const text = pasted.trim();
  const found = /\/invite\/([^/?#\s]+)/.exec(text);
  return found?.[1] === undefined ? text : decodeURIComponent(found[1]);
}

function JoinDialog({ open, onClose }: DialogProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const form = useForm({ link: '' }, { link: (value) => (value.trim() === '' ? t('places.rules.linkMissing') : null) });
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('places.join.title')}
      description={t('places.join.lead')}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button type="submit" form="join-org" variant="primary">
            {t('places.join.submit')}
          </Button>
        </>
      }
    >
      <form
        id="join-org"
        noValidate
        className="py-2"
        onSubmit={form.submit((values) => {
          onClose();
          void navigate({ to: '/invite/$token', params: { token: tokenOf(values.link) } });
        })}
      >
        <Input label={t('places.join.field')} hint={t('places.join.hint')} autoComplete="off" spellCheck={false} {...form.field('link')} />
      </form>
    </Dialog>
  );
}

// ---------------------------------------------------------------- changing

/** A dialog that needs the node itself before it can begin: its revision, its description, who is to be asked. */
function WithNode({
  place,
  children,
}: {
  place: PlaceRef;
  children: (node: Node) => ReactNode;
}) {
  const query = useQuery(nodeQuery(place.id));
  if (query.isPending) return <SkeletonRows rows={3} className="py-2" />;
  if (query.isError) {
    return <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  }
  return children(query.data);
}

function RenameDialog({ place, open, onClose }: DialogProps & { place: PlaceRef }) {
  const { t } = useTranslation();
  const named = useNamedPlace(place);
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('places.rename.title', { name: named })}
      description={t('places.rename.lead')}
    >
      <WithNode place={place}>{(node) => <RenameForm node={node} onClose={onClose} />}</WithNode>
    </Dialog>
  );
}

function RenameForm({ node, onClose }: { node: Node; onClose: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const form = useForm(
    { name: node.name, description: node.description },
    { name: (value) => (value.trim() === '' ? t('places.rules.nameMissing') : null) },
  );

  const save = useMutation({
    mutationFn: (values: { name: string; description: string }) => {
      // Only what changed is written: a name that is written is a name that automation leaves alone.
      const name = values.name.trim();
      const description = values.description.trim();
      return updateNode(node.id, {
        expected_revision: current(queryClient, node).revision,
        ...(name !== node.name ? { name } : {}),
        ...(description !== node.description ? { description } : {}),
      });
    },
    onSuccess: onClose,
    onError: (error) => {
      form.refuse(error);
      // Somebody changed it first: how it stands now is read again, and shown, before anything is tried again.
      if (error instanceof ApiError && error.is('conflict.stale_revision')) void queryClient.invalidateQueries({ queryKey: nodeKey(node.id) });
    },
  });

  const unchanged = form.values.name.trim() === node.name && form.values.description.trim() === node.description;
  const placed = save.error instanceof ApiError && save.error.faults().some((fault) => fault.path in form.values);

  return (
    <form
      noValidate
      className="flex flex-col gap-4 py-2"
      onSubmit={form.submit((values) => {
        if (save.isPending) return;
        if (unchanged) onClose();
        else save.mutate(values);
      })}
    >
      <Input label={t('places.fields.name')} autoComplete="off" {...form.field('name')} />
      <Textarea label={t('places.fields.description')} optional {...form.field('description')} />
      {save.error !== null && !placed && <ErrorAlert error={save.error} />}
      <div className="flex flex-wrap items-center justify-end gap-2 pt-1">
        <Button disabled={save.isPending} onClick={onClose}>
          {t('common.cancel')}
        </Button>
        <Button type="submit" variant="primary" pending={save.isPending}>
          {t('places.rename.submit')}
        </Button>
      </div>
    </form>
  );
}

/** The node as the cache holds it now, which a stale answer has replaced; the one the dialog began with otherwise. */
function current(queryClient: ReturnType<typeof useQueryClient>, node: Node): Node {
  return queryClient.getQueryData<Node>(nodeKey(node.id)) ?? node;
}

// ---------------------------------------------------------------- archiving and deleting

function ArchiveDialog({ place, open, onClose }: DialogProps & { place: PlaceRef }) {
  const { t } = useTranslation();
  const named = useNamedPlace(place);
  const authority = useQuery({ ...nodeQuery(place.id), enabled: open });
  const plan = useQuery({ queryKey: ['archive-plan', place.id], queryFn: () => previewArchive(place.id), enabled: open, staleTime: 0, retry: false });
  const archive = useMutation({
    mutationFn: () => {
      if (plan.data === undefined || plan.isError || plan.data.blockers.length > 0) return Promise.reject(new Error(t('placeReview.newReviewRequired')));
      return applyArchivePlan(plan.data);
    },
    onError: (error) => {
      if (error instanceof ApiError && (error.is('conflict.stale_revision') || error.is('preflight.expired'))) void plan.refetch();
    },
    onSuccess: (result) => {
      onClose();
      const inverse = result.undo;
      toast({
        tone: 'success',
        title: t('places.toast.archived', { name: place.name }),
        ...(inverse === null ? {} : { action: {
          label: t('places.toast.undo'),
          run: () => void applyArchivePlan(inverse).catch(() => toast({ tone: 'danger', title: t('placeReview.undoChanged') })),
        } }),
      });
    },
  });
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('places.archive.title', { name: named })}
      admin={adminSpec(authority.data, `${place.kind}.archive`)}
      confirmLabel={t('places.archive.confirm', { name: place.name })}
      pending={archive.isPending}
      disabled={plan.isPending || plan.isFetching || plan.isError || plan.data.blockers.length > 0}
      error={archive.error ?? undefined}
      onConfirm={() => archive.mutate()}
    >
      {plan.isError ? <ErrorState compact error={plan.error} onRetry={() => void plan.refetch()} /> : plan.data === undefined ? <SkeletonRows rows={4} /> : <ArchiveConsequences plan={plan.data} />}
    </ConfirmDialog>
  );
}

/** What lies under a place in the trees that are held, or `null` while none holds it. */
function useCountsUnder(place: PlaceRef): SubtreeCounts | null {
  const byOrg = useTreeStore((state) => state.byOrg);
  return useMemo(() => {
    for (const tree of byOrg.values()) if (tree.nodes.has(place.id)) return subtreeCounts(tree.nodes, place.id);
    return null;
  }, [byOrg, place.id]);
}

/** What a place's deletion takes with it, in words. */
function whatGoes(counts: SubtreeCounts | null, t: TFunction): string {
  if (counts === null) return '';
  const parts = [
    counts.orgs > 0 ? t('places.counts.orgs', { count: counts.orgs }) : null,
    counts.projects > 0 ? t('places.counts.projects', { count: counts.projects }) : null,
    counts.folders > 0 ? t('places.counts.folders', { count: counts.folders }) : null,
    counts.topics > 0 ? t('places.counts.topics', { count: counts.topics }) : null,
    counts.threads > 0 ? t('places.counts.threads', { count: counts.threads }) : null,
  ].filter((part) => part !== null);
  return parts.join(t('places.counts.join'));
}

/** What a reviewed deletion removes besides the place itself, in words, documents among it: the server's counts, which see what the tree may not hold. */
function whatThePlanRemoves(plan: NodeDeletePlan, kind: PlaceRef['kind'], t: TFunction): string {
  const besides = (count: number, own: boolean) => Math.max(0, own ? count - 1 : count);
  const counts = plan.counts;
  const parts = [
    besides(counts.orgs, kind === 'org') > 0 ? t('places.counts.orgs', { count: besides(counts.orgs, kind === 'org') }) : null,
    besides(counts.projects, kind === 'project') > 0 ? t('places.counts.projects', { count: besides(counts.projects, kind === 'project') }) : null,
    besides(counts.topics, kind === 'topic') > 0 ? t('places.counts.topics', { count: besides(counts.topics, kind === 'topic') }) : null,
    besides(counts.threads, kind === 'thread') > 0 ? t('places.counts.threads', { count: besides(counts.threads, kind === 'thread') }) : null,
    counts.documents > 0 ? t('places.counts.documents', { count: counts.documents }) : null,
  ].filter((part) => part !== null);
  return parts.join(t('places.counts.join'));
}

/** The plan as it now is, where a deletion was refused because what it would remove changed since its plan. */
function staleCurrent(error: unknown): NodeDeletePlan | null {
  if (!(error instanceof ApiError) || !error.is('conflict.stale_revision')) return null;
  const current: unknown = (error.problem as unknown as Record<string, unknown>).current;
  return typeof current === 'object' && current !== null && 'preflight_id' in current && 'counts' in current ? (current as NodeDeletePlan) : null;
}

const ROLE_CHOICES: readonly Role[] = ['viewer', 'interactor', 'developer', 'admin'];

/**
 * Whether deleting the place deletes an org that lies in another. Only such an org has an org above it for its
 * members and its resources to go to; a root org has none, and neither has anything that is no org.
 */
export function deletesDepartment(place: Pick<PlaceRef, 'kind' | 'parentId'>, node: Node | undefined): boolean {
  if (place.kind !== 'org') return false;
  return node?.kind === 'org' ? node.parent_id !== null : place.parentId !== null;
}

function DeleteDialog({ place, open, onClose }: DialogProps & { place: PlaceRef }) {
  const { t } = useTranslation();
  const named = useNamedPlace(place);
  const describe = useErrorText();
  const navigate = useNavigate();
  const heading = useCurrentHeading();
  const counts = useCountsUnder(place);
  const running = counts?.running ?? 0;
  const [role, setRole] = useState<Role>('viewer');
  const [resources, setResources] = useState<'disable' | 'to_parent'>('disable');
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  // The server's plan of what goes, until when and by whom it comes back; the deletion names it, and a
  // refusal because it changed brings the plan as it now is, to be confirmed again.
  const planned = useQuery({
    queryKey: ['delete-plan', place.id],
    queryFn: () => previewDelete(place.id),
    enabled: open,
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
    retry: false,
    refetchOnWindowFocus: false,
  });
  const [replaced, setReplaced] = useState<NodeDeletePlan | null>(null);
  const plan = replaced ?? planned.data ?? null;
  const what = plan === null ? whatGoes(counts, t) : whatThePlanRemoves(plan, place.kind, t);
  const node = useQuery({ ...nodeQuery(place.id), enabled: open });
  const isDepartment = deletesDepartment(place, node.data);
  const parent = useTreeStore((state) => {
    if (place.parentId === null) return null;
    for (const tree of state.byOrg.values()) {
      const found = tree.nodes.get(place.parentId);
      if (found !== undefined) return found;
    }
    return null;
  });

  const remove = useMutation({
    mutationFn: () =>
      deleteNode(place.id, {
        ...(isDepartment ? { members: { role }, resources } : {}),
        ...(plan === null ? {} : { preflight_id: plan.preflight_id, expected_revision: plan.revision }),
      }),
    onError: (error) => {
      const current = staleCurrent(error);
      if (current !== null) setReplaced(current);
    },
    onSuccess: (deleted) => {
      // A screen that was in what is deleted has nothing left to show: it goes to the place above. What it had read
      // of what is gone is not asked again: the registry of acts leaves that to the tree, which says what is gone.
      const viewing = heading?.nodeId;
      const inside = viewing !== undefined && useTreeStore.getState().byOrg.size > 0 && isInside(viewing, place.id);
      onClose();
      if (inside && parent !== null) void navigate({ to: placePath({ id: parent.id, kind: parent.kind }) });
      toast({
        tone: 'success',
        title: t('places.toast.deleted', { name: place.name }),
        // The Undo of this deletion puts back how each thing stood, archived or not; a newer deletion or a change
        // in between refuses it, and nothing is half brought back.
        action: {
          label: t('places.toast.undo'),
          run: () =>
            void restoreNode(place.id, deleted.deletion_id)
              .catch((error: unknown) => toast({ tone: 'danger', title: t('places.toast.notRestored', { name: place.name }), description: describe(error).message })),
        },
      });
    },
  });

  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('places.delete.title', { name: named })}
      admin={adminSpec(node.data, `${place.kind}.manage`)}
      confirmLabel={t('places.delete.confirm', { name: place.name })}
      danger
      pending={remove.isPending}
      error={staleCurrent(remove.error) === null ? (remove.error ?? undefined) : undefined}
      onConfirm={() => remove.mutate()}
    >
      <div className="flex flex-col gap-3">
        {staleCurrent(remove.error) !== null && <InlineAlert tone="warning">{t('places.delete.changed')}</InlineAlert>}
        <p>{what === '' ? t('places.delete.bodyEmpty', { name: place.name }) : t('places.delete.body', { name: place.name, what })}</p>
        {running > 0 && <p>{t('places.delete.running', { count: running })}</p>}
        {plan === null ? (
          <p>{t('places.delete.recoverable')}</p>
        ) : (
          <p>
            {t(plan.restore_by.permission === 'access.own' ? 'places.delete.untilOwners' : 'places.delete.until', {
              date: formatExact(plan.purge_at, { locale, timeZone }),
              place: plan.restore_by.place === 'system' ? t('places.delete.theSystem') : (plan.restore_by.place.name ?? ''),
            })}
          </p>
        )}
        {planned.isError && replaced === null && <ErrorAlert error={planned.error} />}
        {isDepartment && node.data?.kind === 'org' && parent !== null && (
          <div className="flex flex-col gap-3 border-t border-border pt-3">
            <Select
              label={t('places.delete.members', { count: node.data.member_count, parent: parent.name })}
              value={role}
              onValueChange={setRole}
              options={ROLE_CHOICES.map((value) => ({ value, ...choiceOption(t, `accessRole.${value}`) }))}
            />
            <Select
              label={t('places.delete.resources')}
              value={resources}
              onValueChange={setResources}
              options={[
                { value: 'disable', ...choiceOption(t, 'departmentResources.disable') },
                { value: 'to_parent', ...choiceOption(t, 'departmentResources.to_parent', { parent: parent.name }) },
              ]}
            />
          </div>
        )}
      </div>
    </ConfirmDialog>
  );
}

/** Whether the node in view is the place or lies under it, from the trees held. */
function isInside(nodeId: string, placeId: string): boolean {
  for (const tree of useTreeStore.getState().byOrg.values()) {
    if (tree.nodes.has(nodeId)) return pathTo(tree, nodeId).some((node) => node.id === placeId);
  }
  return false;
}
