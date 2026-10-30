/** The actual readable folder path of a topic, including truthful nonlinks for concealed ancestors. */
import { useTranslation } from 'react-i18next';
import { ChevronRight } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';

import { nodeQuery } from '@/api/nodes';
import { PlaceLink } from '@/features/navigation/places';
import type { Topic } from '@/generated/api-types';

function OrgPart({ id, name }: { id: string; name: string }) {
  const node = useQuery(nodeQuery(id));
  if (node.data?.kind !== 'org') return <span>{name}</span>;
  return <a href={`/org/${encodeURIComponent(node.data.address)}`} className="rounded-sm wrap-anywhere hover:underline">{name}</a>;
}

export function TopicPath({ topic }: { topic: Pick<Topic, 'path'> }) {
  const { t } = useTranslation();
  const path = topic.path.filter((place) => place.kind === 'org' || place.kind === 'project');
  return <span className="flex flex-wrap items-center gap-x-1 text-xs text-text-muted">{path.map((place, at) => <span key={place.id} className="inline-flex min-w-0 items-center gap-1">{at > 0 && <ChevronRight className="size-4" aria-hidden />}{place.redacted || place.name === null ? <span>{t('access.place.hidden')}</span> : place.kind === 'org' ? <OrgPart id={place.id} name={place.name} /> : <PlaceLink place={place} className="rounded-sm wrap-anywhere hover:underline">{place.name}</PlaceLink>}</span>)}</span>;
}
