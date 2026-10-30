/** An org's Everything starts the shared visible hierarchy at that org, for every reader of its settings. */
import { useTranslation } from 'react-i18next';

import { ContentsTree } from '@/features/admin/ContentsTree';
import type { Org } from '@/generated/api-types';
import { Section } from '@/shell/Page';

export function OrgEverything({ org }: { org: Org }) {
  const { t } = useTranslation();
  return <Section id="org-everything" title={t('everything.title')}>
    <ContentsTree orgId={org.id} q="" />
  </Section>;
}
