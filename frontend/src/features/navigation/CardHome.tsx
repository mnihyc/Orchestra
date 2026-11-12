/** A card's home follows the same reader-filtered breadcrumb path as the home page. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import type { NodeRef } from '@/generated/api-types';
import { linkTo } from '@/lib/links';
import { Breadcrumbs } from '@/ui/Breadcrumbs';

export function cardHome(value: unknown): NodeRef | null {
  if (typeof value !== 'object' || value === null || !('id' in value) || typeof value.id !== 'string'
    || !('kind' in value) || !('name' in value) || !(value.name === null || typeof value.name === 'string')
    || !('redacted' in value) || typeof value.redacted !== 'boolean') return null;
  const { kind } = value;
  if (kind !== 'org' && kind !== 'project' && kind !== 'topic' && kind !== 'thread') return null;
  return { id: value.id, kind, name: value.name, redacted: value.redacted };
}

export function CardHome({ place }: { place: NodeRef }) {
  const { t } = useTranslation();
  const home = useQuery({ ...nodeQuery(place.id), enabled: !place.redacted && place.name !== null, retry: false });
  const path = home.isError || place.redacted || !home.data?.path.length ? [place] : home.data.path;
  return <div data-card-home="" className="min-w-0">
    <Breadcrumbs className="[&_ol]:text-sm" crumbs={path.map((node) => ({
      id: node.id,
      label: node.redacted || node.name === null ? t('ui.nameUnknown') : node.name,
      ...(node.redacted || node.name === null ? {} : { href: linkTo({ kind: node.kind, id: node.id }) }),
    }))} />
  </div>;
}
