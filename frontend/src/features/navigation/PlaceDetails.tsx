/** Descriptions and tags on demand for the compact headers of orgs, departments and projects. */
import { Link } from '@tanstack/react-router';
import { FileText, Settings } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { placeOfNode } from '@/features/navigation/places';
import { TagPicker } from '@/features/topic/TagPicker';
import type { Org, Project } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';
import { Tooltip } from '@/ui/Tooltip';

type Place = Org | Project;

/** Row details include their description; a page already shows it beside its heading. */
export function PlaceDetails({ node, row = false }: { node: Place; row?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const mayEdit = holds(node.access, `${node.kind}.manage`) && !node.archived;
  const label = row ? t('places.details.named', { name: node.name }) : t('places.details.title');
  if (!mayEdit && node.description === '' && node.tags.length === 0) return null;
  return <Popover open={open} onOpenChange={setOpen} label={label}
    trigger={<IconButton size="sm" label={label} icon={<FileText aria-hidden />} />}
    className="flex w-[min(22rem,calc(100vw-2rem))] flex-col gap-3">
    <PlaceDetailsContents node={node} onEdit={() => setOpen(false)} showDescription={row || node.description === ''} />
  </Popover>;
}

/** The same complete metadata in a row's popover or the page menu's reading dialog. */
export function PlaceDetailsContents({ node, onEdit, showDescription = true }: { node: Place; onEdit?: () => void; showDescription?: boolean }) {
  const { t } = useTranslation();
  const mayEdit = holds(node.access, `${node.kind}.manage`) && !node.archived;
  const mayTag = holds(node.access, `${node.kind}.interact`) && !node.archived;
  return <div className="flex min-w-0 flex-col gap-3">
    {showDescription && <p className="whitespace-pre-wrap wrap-anywhere text-text">{node.description === '' ? t('places.details.empty') : node.description}</p>}
    <TagPicker place={node} allowed={mayTag} />
    {mayEdit ? <Button size="sm" action="edit" className="self-start" onClick={() => { onEdit?.(); openNodeDialog({ type: 'rename', node: placeOfNode(node) }); }}>{t('places.details.edit')}</Button>
      : !node.archived && <p className="text-sm wrap-anywhere text-text-muted">{t('places.details.ask', { name: node.owner.display_name })}</p>}
  </div>;
}

/** A named, native settings link has the same compact hit area as the other header icons. */
export function PlaceSettingsLink({ node }: { node: Place }) {
  const { t } = useTranslation();
  const label = t('places.org.settings');
  const look = 'focus-inset inline-flex size-(--control-sm) shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-muted hover:text-text';
  return <Tooltip content={label}>{node.kind === 'org'
    ? <Link to="/org/$slug/settings/$section" params={{ slug: node.address, section: 'general' }} aria-label={label} className={look}><Settings aria-hidden /></Link>
    : <Link to="/project/$projectId/settings" params={{ projectId: node.id }} aria-label={label} className={look}><Settings aria-hidden /></Link>}
  </Tooltip>;
}
