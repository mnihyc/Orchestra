/** One Outside Orch section with the applicable people and room/system lists, retaining each list's existing guard. */
import { useTranslation } from 'react-i18next';

import { Destinations } from '@/features/settings/Destinations';
import { Channels } from '@/features/settings/org/Channels';
import type { Org, Project, Topic } from '@/generated/api-types';
import { Section } from '@/shell/Page';

export function OutsideOrch({ node }: { node: Org | Project | Topic }) {
  const { t } = useTranslation();
  return <Section id="outside-orch" title={t('day100.names.outside')}>
    {node.kind === 'org' && <Channels org={node} level={3} />}
    <Destinations node={node} level={3} />
  </Section>;
}
