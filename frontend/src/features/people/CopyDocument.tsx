/** A current readable source copied to an eligible place, with fresh ownership and access explained first. */
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { Copy } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { documentQuery } from '@/api/documents';
import { nodeQuery, orgsQuery } from '@/api/nodes';
import { copyDocument } from '@/api/people';
import { settingsQuery } from '@/api/settings';
import { NamePicker } from '@/features/search/NamePicker';
import type { Document, TreeNode } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { useOrgTreeFeed } from '@/realtime/live';
import { pathTo, useOrgTree } from '@/realtime/stores/tree';
import { useSelectedOrg } from '@/shell/org';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Tree, type TreeRow } from '@/ui/Tree';
import { Select } from '@/ui/Select';

export function CopyDocumentDialog({ source, destination, onClose }: { source?: Document | null; destination: string; onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const key = useActKey();
  const selected = useSelectedOrg((state) => state.orgId);
  const [rootChoice, setRootChoice] = useState<string | null>(null);
  const [sourceId, setSourceId] = useState(source?.id ?? null);
  const [target, setTarget] = useState(destination);
  const [priv, setPrivate] = useState(false);
  const [closed, setClosed] = useState<Set<string>>(new Set());
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const read = useQuery({ ...documentQuery(sourceId ?? ''), enabled: sourceId !== null, retry: false, staleTime: 0 });
  const place = useQuery({ ...nodeQuery(target), retry: false, staleTime: 0 });
  const orgs = useQuery(orgsQuery);
  const settings = useQuery(settingsQuery(target));
  const root = rootChoice ?? place.data?.path[0]?.id ?? source?.path[0]?.id ?? selected;
  useOrgTreeFeed(root);
  const tree = useOrgTree(root);
  const disabled = settings.data?.find((setting) => setting.key === 'docs.enabled')?.value === false;
  const rows = useMemo<TreeRow<TreeNode>[]>(() => {
    const nodes = [...tree.nodes.values()].filter((node) => !node.deleted);
    const visible = nodes.filter((node) => !pathTo(tree, node.id).slice(0, -1).some((ancestor) => closed.has(ancestor.id)));
    return visible.sort((a, b) => pathTo(tree, a.id).map((part) => part.name).join('/').localeCompare(pathTo(tree, b.id).map((part) => part.name).join('/'))).map((node) => ({ id: node.id, item: node, depth: Math.max(0, pathTo(tree, node.id).length - 1), parentId: node.parent_id, expandable: nodes.some((child) => child.parent_id === node.id), expanded: !closed.has(node.id), tick: target === node.id, tickLocked: node.archived }));
  }, [tree, closed, target]);
  const canCreate = place.isSuccess && holds(place.data.access, 'doc.create') && !place.data.archived && !disabled;
  const allowed = read.isSuccess && !read.isFetching && canCreate && settings.isSuccess;
  const act = async () => {
    if (!allowed || pending || sourceId === null) return;
    setPending(true); setError(null);
    const body = { source_document_id: sourceId, access: priv ? 'private' as const : 'follows' as const, client_id: key({ sourceId, target, priv }) };
    try {
      const made = await copyDocument(target, body);
      onClose();
      void navigate({ to: '/doc/$documentId', params: { documentId: made.id }, search: { new: true } });
    } catch (failure) { setError(failure); }
    finally { setPending(false); }
  };
  return <Dialog open onOpenChange={(next) => !next && !pending && onClose()} title={t('people.copyTitle')} description={t('people.copyAbout')} size="lg" dismissible={!pending} footer={<>
    <Button variant="ghost" onClick={onClose} disabled={pending}>{t('common.cancel')}</Button>
    <Button variant="primary" icon={<Copy aria-hidden />} pending={pending} disabled={!allowed} onClick={() => void act()}>{t('people.createCopy')}</Button>
  </>}>
    <div className="flex flex-col gap-3">
      <NamePicker label={t('people.source')} kinds={['document']} chosen={read.isSuccess ? read.data.title : null} anything={t('people.source')} placeholder={t('people.source')} nothing={t('people.noReaders')} onChoose={(hit) => setSourceId(hit?.id ?? null)} />
      {read.isSuccess && <p className="text-sm text-text-muted">{t('people.copySource', { title: read.data.title })}</p>}
      {sourceId !== null && read.isPending && <p role="status">{t('common.loading')}</p>}
      {read.isError && <ErrorAlert error={read.error} />}
      <p className="text-sm font-medium">{t('people.destination')}: {place.isSuccess ? place.data.name : t('common.loading')}</p>
      {orgs.isSuccess && root !== null && <Select<string> label={t('people.destinationOrg')} value={root} options={orgs.data.map((org) => ({ value: org.id, ...choiceOption(t, 'dynamic.place', { name: org.name }) }))} onValueChange={(next) => { setRootChoice(next); setTarget(next); }} />}
      <div className="max-h-60 overflow-y-auto rounded-lg border border-border"><Tree label={t('people.destination')} rows={rows} currentId={target} onExpandedChange={(id, expanded) => setClosed((before) => new Set(expanded ? [...before].filter((one) => one !== id) : [...before, id]))} onTick={(row, on) => on && setTarget(row.id)} nameOf={(node) => node.name} renderRow={(row) => <span className="flex min-w-0 flex-1 items-center gap-2 py-1"><span className="truncate">{row.item.name}</span>{row.item.archived && <span className="text-xs text-text-muted">{t('documents.list.archived')}</span>}</span>} /></div>
      {place.isError && <ErrorAlert error={place.error} />}
      {settings.isError && <ErrorAlert error={settings.error} />}
      {place.isSuccess && !canCreate && <p className="text-sm text-text-muted">{disabled ? t('people.documentsDisabled') : t('people.destinationNeedsCreate')}</p>}
      <Checkbox label={t(priv ? 'people.copyPrivateLabel' : 'people.copyFollowsLabel')} checked={priv} onCheckedChange={setPrivate} />
      {error !== null && <ErrorAlert error={error} />}
    </div>
  </Dialog>;
}
