/** Historical sharing wording uses the supplied readable thing, its actual areas and levels. */
import type { TFunction } from 'i18next';
import { AREA_WORDS, stepWords } from '@/features/access/AgentRow';
import { AREAS, stepOf } from '@/features/access/agentShares';
import type { MarkerShare as NamedSharedMarker } from '@/generated/api-types';

export function namedSharedSentence(data: NamedSharedMarker, t: TFunction): string | null {
  const what = data.what;
  const target = 'node' in what ? what.node : 'chat' in what ? what.chat : what.document;
  if ('redacted' in target && target.redacted) return null;
  const name = 'title' in target ? target.title : target.name;
  if (name === null) return null;
  const parts = data.parts ?? {};
  const areas = AREAS.filter((area) => stepOf(area, parts) > 0).map((area) => `${t(AREA_WORDS[area])}: ${stepWords(t, area, stepOf(area, parts))}`).join(t('startFlow.separator'));
  return `${t('people.sharedNamed', { by: data.by.display_name, name })}${areas === '' ? '' : ` (${areas})`}`;
}
