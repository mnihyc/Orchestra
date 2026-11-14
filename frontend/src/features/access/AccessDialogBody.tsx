/** The dialog behind Share: one thing's page, with a place's People, Defaults and Agents beside it. Fetched when a page is first asked for (see `AccessDialog.tsx`). */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessTarget, isPlaceKind, pageQuery, permissionsQuery } from '@/api/access';
import { nodeQuery } from '@/api/nodes';
import { registerRowOpener } from '@/features/finder/pageRows';
import { useNodeLabel } from '@/features/navigation/placeLabels';
import { GuideLink } from '@/features/help/GuideLink';
import { Dialog } from '@/ui/Dialog';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tabs } from '@/ui/Tabs';

import { type AccessTab, EXIT_MS, openAccess } from './AccessDialog';
import { Agents } from './Agents';
import { ChatAccessPanel } from './GroupAccess';
import { ByArea } from './ByArea';
import { type Asked, ChangePreview } from './ChangePreview';
import { type PageSource, useChanges } from './changes';
import { Defaults } from './Defaults';
import { indexCatalogue } from './model';
import { Roles } from './Roles';
import { ShortForm } from './ShortForm';
import { containerOf } from './model';
import { ViewAs } from './ViewAs';

/** The page of one thing: the short form or the full one, and for a place its People and Defaults. */
function PagePanel({ target, share, active, initialPermission }: { target: AccessTarget; share: boolean; active: boolean; initialPermission: string | null }) {
  const { t } = useTranslation();
  const catalogue = useQuery(permissionsQuery);
  const page = useQuery(pageQuery(target));
  const source: PageSource = { type: 'page', target };
  const changes = useChanges(source);
  const [asked, setAsked] = useState<Asked | null>(null);
  const [full, setFull] = useState(initialPermission !== null);
  const [viewAs, setViewAs] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  const [jump, setJump] = useState<{permission:string} | null>(initialPermission === null ? null : { permission: initialPermission });
  useEffect(() => {
    if (!active || page.isError || page.data === undefined) return;
    return registerRowOpener((permission) => { setFull(true); setJump({permission}); },page.data.rows.map((row) => row.permission));
  },[active,page.data,page.isError]);
  useEffect(() => {
    if (jump === null || !full || page.data === undefined || catalogue.data === undefined) return;
    const row = [...(body.current?.querySelectorAll<HTMLElement>('[data-access-permission]') ?? [])].find((row) => row.dataset.accessPermission === jump.permission);
    if (row === undefined) return;
    for (let parent = row.parentElement; parent !== null; parent=parent.parentElement) if (parent instanceof HTMLDetailsElement) parent.open=true;
    // Let the finder finish its normal exit/focus return before moving the keyboard to this row.
    const focus = setTimeout(() => { if (row.isConnected) { row.scrollIntoView({block:'center'}); row.focus(); } },EXIT_MS);
    row.classList.add('outline','outline-2','outline-focus','rounded-md');
    const clear = setTimeout(() => row.classList.remove('outline','outline-2','outline-focus','rounded-md'),2_000);
    return () => { clearTimeout(focus); clearTimeout(clear); row.classList.remove('outline','outline-2','outline-focus','rounded-md'); };
  },[jump,full,page.data,catalogue.data]);

  if (page.isError) return <ErrorState compact error={page.error} title={t('access.page.notLoaded')} retrying={page.isRefetching} onRetry={() => void page.refetch()} />;
  if (catalogue.isError) return <ErrorState compact error={catalogue.error} retrying={catalogue.isRefetching} onRetry={() => void catalogue.refetch()} />;
  if (page.isPending || catalogue.isPending) return <SkeletonRows reads={[page, catalogue]} rows={7} />;

  const index = indexCatalogue(catalogue.data);
  return (
    <div ref={body} className="contents">
      {full ? (
        <>
          <ByArea target={target} page={page.data} catalogue={index} source={source} changes={changes} ask={setAsked} onBack={() => setFull(false)} />
          <Button size="sm" variant="ghost" action="viewAs" onClick={() => setViewAs(true)}>{t('access.footer.viewAs')}</Button>
        </>
      ) : (
        <ShortForm
          target={target}
          page={page.data}
          catalogue={index}
          source={source}
          changes={changes}
          ask={setAsked}
          onFull={() => setFull(true)}
          focusAdd={share}
          onOpen={(next) => openAccess(next, { share: false })}
        />
      )}
      <ChangePreview source={source} asked={asked} changes={changes} onClose={() => setAsked(null)} />
      {viewAs && <ViewAs target={target} open onClose={() => setViewAs(false)} />}
    </div>
  );
}

export interface AccessDialogProps {
  target: AccessTarget;
  open: boolean;
  onClose: () => void;
  initialTab?: AccessTab;
  /** Opened by Share: the field that adds people has the keyboard. */
  share?: boolean;
  /** A question from a place's empty state opens and focuses this permission's existing By area row. */
  initialPermission?: string | null;
}

export function AccessDialog({ target, open, onClose, initialTab = 'page', share = true, initialPermission = null }: AccessDialogProps) {
  const { t } = useTranslation();
  const place = isPlaceKind(target.kind);
  const [tab, setTab] = useState<AccessTab>(initialTab);
  const page = useQuery({ ...pageQuery(target), enabled: target.kind === 'project' });
  const node = useQuery({ ...nodeQuery(target.id), enabled: target.kind === 'project' }).data;
  const parent = page.data === undefined ? null : containerOf(page.data);
  const folder = target.kind === 'project' && (node === undefined ? parent !== null && parent !== 'system' && parent.kind === 'project' : node.path.at(-2)?.kind === 'project');
  const kind = folder ? t('craftAccess.folder') : t(`access.kinds.${target.kind}`, { defaultValue: target.kind });
  // A place is named with where it is when another place the reader sees has its name: "Access to “dep2 · Acme”".
  const called = useNodeLabel(target.id, target.name) ?? target.name;

  const body = (() => {
    if (!place && target.kind !== 'thread' && target.kind !== 'document' && target.kind !== 'chat') return <PagePanel target={target} share={share} active={open} initialPermission={initialPermission} />;
    return (
      <Tabs<AccessTab>
        label={t('access.tabs.label')}
        value={tab}
        onValueChange={setTab}
        panelClassName="pt-4"
        tabs={[
          { value: 'page', label: place ? t('access.tabs.this', { kind }) : t('access.tabs.access'), panel: target.kind === 'chat' ? <ChatAccessPanel id={target.id} fallback={<PagePanel target={target} share={share} active={open && tab === 'page'} initialPermission={initialPermission} />} /> : <PagePanel target={target} share={share} active={open && tab === 'page'} initialPermission={initialPermission} /> },
          ...(place
            ? [
                { value: 'people' as const, label: t('access.tabs.people'), panel: <Roles place={target} /> },
                { value: 'defaults' as const, label: t('access.tabs.defaults'), panel: <Defaults place={target} /> },
              ]
            : []),
          { value: 'agents', label: t('access.tabs.agents'), panel: <Agents target={target} /> },
        ]}
      />
    );
  })();

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()} size={tab === 'page' ? 'md' : 'lg'} title={t('access.title', { name: called })} footer={<GuideLink guide="who-can-see-and-do-what" />}>
      {/* A dialog is as wide as what is in it, up to its limit, and what is here has long names that would make it wider than a phone: the page is as wide as the dialog lets it be, and what cannot shrink scrolls inside. */}
      <div className="w-0 min-w-full" data-tutorial-target="permission-page">{body}</div>
    </Dialog>
  );
}
