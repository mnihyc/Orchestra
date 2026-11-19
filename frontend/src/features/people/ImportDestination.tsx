/** An import destination is checked against the actual place and document-folder standing. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { documentDirectoryQuery, documentTreeEntryQuery } from '@/api/documentTree';
import { nodeQuery } from '@/api/nodes';
import { ImportFolderBrowser } from '@/features/people/ImportFolderBrowser';
import { NamePicker } from '@/features/search/NamePicker';
import type { ImportRoot, ImportSourceMode, NameHit, Node } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

/** A selected name and its actual identity. */
export interface ImportDestinationName { id: string; label: string }

/** The choices made before the server plans the import. */
export interface ImportDestinationValue {
  place: ImportDestinationName | null;
  folder: ImportDestinationName | null;
  root: ImportRoot;
}

type DestinationReason = 'archived' | 'denied' | 'folderDenied' | 'wrongFolder';

/** The actual current creation decision, not a role inferred in the picker. */
export interface ImportDestinationStanding {
  node: Node | undefined;
  checking: boolean;
  allowed: boolean;
  reason: DestinationReason | null;
  error: unknown;
  retry: () => void;
}

/** Reads the destination and its document directory, including docs policy and archived state. */
export function useImportDestinationStanding(value: ImportDestinationValue, mode: ImportSourceMode | null): ImportDestinationStanding {
  const cache = useQueryClient();
  const placeId = value.place?.id ?? '';
  const folderId = value.folder?.id ?? '';
  const node = useQuery({ ...nodeQuery(placeId), enabled: placeId !== '' });
  const root = useQuery({ ...documentDirectoryQuery({ node: placeId, at: { kind: 'node_docs', id: placeId } }, cache), enabled: placeId !== '' && folderId === '' });
  const folder = useQuery({ ...documentTreeEntryQuery({ kind: 'container', id: folderId }, cache), enabled: placeId !== '' && folderId !== '' });
  const directory = folderId === '' ? root.data?.directory : folder.data?.directory;
  const readDirectory = folderId === '' ? root : folder;
  const current = node.isError ? undefined : node.data;
  const hierarchy = current !== undefined && current.kind !== 'thread' && (holds(current.access, `${current.kind}.interact`)
    || current.kind === 'org' && holds(current.access, 'org.manage') || holds(current.access, 'chat.interact'));
  const needsDirectory = mode === 'files' || folderId !== '' || !hierarchy;
  const checking = placeId !== '' && (node.isPending || needsDirectory && readDirectory.isPending);
  const error = node.isError ? node.error : needsDirectory && readDirectory.isError ? readDirectory.error : null;
  let reason: DestinationReason | null = null;
  if (current?.archived === true) reason = 'archived';
  else if (current?.kind === 'thread') reason = 'denied';
  else if (directory !== undefined && directory !== null && directory.node.id !== placeId) reason = 'wrongFolder';
  else if (folderId !== '' && directory !== undefined && directory !== null && !directory.can_create) reason = 'folderDenied';
  else if (current !== undefined && !checking && error === null && !hierarchy && directory?.can_create !== true) reason = 'denied';
  else if (mode === 'files' && !checking && error === null && directory?.can_create !== true) reason = 'folderDenied';
  const allowed = placeId !== '' && current !== undefined && current.kind !== 'thread' && !checking && error === null && reason === null
    && (mode === 'files' ? directory?.can_create === true : hierarchy || directory?.can_create === true);
  return { node: current, checking, allowed, reason, error, retry: () => { if (node.isError) void node.refetch(); if (readDirectory.isError) void readDirectory.refetch(); } };
}

/** A place path for the picker, without revealing a redacted ancestor. */
function placeLabel(hit: NameHit, unknown: string): string {
  const names = hit.path.map((part) => part.redacted ? unknown : part.name ?? unknown);
  if (hit.path.at(-1)?.id !== hit.id) names.push(hit.name);
  return names.join(' / ');
}

export function ImportDestination({ value, onChange, standing, isPackage }: {
  value: ImportDestinationValue;
  onChange: (value: ImportDestinationValue) => void;
  standing: ImportDestinationStanding;
  isPackage: boolean;
}) {
  const { t } = useTranslation();
  const placeId = value.place?.id;
  const path = standing.node?.path.map((part) => part.redacted ? t('ui.nameUnknown') : part.name ?? t('ui.nameUnknown')).join(' / ');
  const placeName = standing.node === undefined && standing.error !== null ? t('ui.nameUnknown') : path === undefined || path === '' ? value.place?.label ?? '' : path;
  return <div className="flex min-w-0 flex-col gap-4">
    <NamePicker label={t('importWizard.destination.place')} kinds={['org', 'project', 'topic']} chosen={value.place === null ? null : placeName}
      placeholder={t('importWizard.destination.findPlace')} nothing={t('importWizard.destination.noPlace')} accepts={(hit) => !hit.archived}
      onChoose={(hit) => onChange({ ...value, place: hit === null ? null : { id: hit.id, label: placeLabel(hit, t('ui.nameUnknown')) }, folder: null })} />
    {value.place !== null && <div className="flex min-w-0 flex-wrap items-end gap-2">
      <div className="min-w-0 flex-1"><NamePicker label={t('importWizard.destination.folder')} kinds={['document_container']} chosen={value.folder?.label ?? null}
        anything={t('importWizard.destination.folderDefault', { place: placeName })}
        orgId={standing.node?.org_id} accepts={(hit) => !hit.archived && hit.path.at(-1)?.id === placeId}
        placeholder={t('importWizard.destination.findFolder')} nothing={t('importWizard.destination.noFolder')}
        onChoose={(hit) => onChange({ ...value, folder: hit === null ? null : { id: hit.id, label: hit.name } })} /></div>
      <ImportFolderBrowser key={value.place.id} place={value.place.id} onChoose={(folder) => onChange({ ...value, folder })} />
    </div>}
    {isPackage && <SegmentedControl<ImportRoot> label={t('importWizard.destination.root')} value={value.root} onValueChange={(root) => onChange({ ...value, root })}
      segments={[
        { value: 'inside', label: t('importWizard.destination.inside'), description: t('importWizard.destination.insideAbout') },
        { value: 'here', label: t('importWizard.destination.here'), description: t('importWizard.destination.hereAbout') },
      ]} />}
    {value.place !== null && (standing.checking ? <SkeletonRows rows={1} /> : standing.error !== null ? <ErrorAlert error={standing.error} action={<Button size="sm" onClick={standing.retry}>{t('common.tryAgain')}</Button>} />
      : <StateLine compact role="status" tone={standing.reason === null ? 'neutral' : 'warning'} message={t(standing.reason === null ? 'importWizard.destination.allowed' : `importWizard.destination.${standing.reason}`)} />)}
  </div>;
}
