/** A report's named object opens the shared reference card, which checks current access on demand. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { FileText, Folder, MessageSquare } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { documentTreeEntryQuery } from '@/api/documentTree';
import { isPlainClick } from '@/features/chat/open';
import { REFERENCE_CHIP_CLASS } from '@/features/people/NamedChip';
import type { TypedReference } from '@/features/people/references';
import type { PackageKind } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { NodeIcon } from '@/ui/NodeIcon';
import { Popover } from '@/ui/Popover';
import { ReferenceCard } from '@/ui/ReferenceCard';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

function FolderReference({ id, name, onOpen, scope }: { id: string; name: string; onOpen?: () => void; scope: string }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const [open, setOpen] = useState(false);
  const folder = useQuery({ ...documentTreeEntryQuery({ kind: 'container', id }, cache, false, scope), enabled: open, staleTime: 0, refetchOnMount: 'always' });
  return <Popover open={open} onOpenChange={setOpen} label={name} trigger={<button type="button" title={name} className={REFERENCE_CHIP_CLASS}><Folder aria-hidden className="size-4 shrink-0" /><span className="min-w-0 truncate">{name}</span></button>}>
    {folder.isPending || folder.isFetching ? <SkeletonRows rows={1} /> : folder.isError ? <ErrorAlert error={folder.error} />
      : folder.data.directory?.folder?.standing.read === true ? <Button asChild><Link to="/document-folder/$folderId" params={{ folderId: id }} onClick={(event) => { if (isPlainClick(event)) onOpen?.(); }}>{t('importWizard.report.openFolder')}</Link></Button>
      : <StateLine compact message={t('documentFolders.unavailable')} />}
  </Popover>;
}

export function ImportReference({ id, kind, name, onOpen }: { id: string; kind: PackageKind; name: string; onOpen?: () => void }) {
  const scope = useStore(client.session, (session) => `${session.userId ?? ''}:${session.readerVersion ?? 0}`);
  // Document folders use their own standing read before offering their route.
  if (kind === 'folder') return <FolderReference id={id} name={name} onOpen={onOpen} scope={scope} />;
  const reference: TypedReference = { id, kind: kind === 'document' ? 'document' : kind === 'chat' ? 'chat' : 'node', label: name };
  const icon = kind === 'document' ? <FileText aria-hidden className="size-4 shrink-0" /> : kind === 'chat' ? <MessageSquare aria-hidden className="size-4 shrink-0" /> : <NodeIcon kind={kind} className="size-4 shrink-0" />;
  const path = kind === 'document' ? `/doc/${encodeURIComponent(id)}` : kind === 'chat' ? `/chat/${encodeURIComponent(id)}` : `/${kind}/${encodeURIComponent(id)}`;
  return <ReferenceCard reference={reference} label={name} onOpen={onOpen} cacheScope={scope}>{(keep) => <a href={path} onClick={(event) => { if (isPlainClick(event)) { event.preventDefault(); keep(); } }} title={name} className={REFERENCE_CHIP_CLASS}>
    {icon}<span className="min-w-0 truncate">{name}</span>
  </a>}</ReferenceCard>;
}
