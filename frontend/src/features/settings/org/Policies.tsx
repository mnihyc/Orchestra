/** Scope policy editor.
 *
 * The settings editor at an org's scope: every key valid here, by group, with where each value comes
 * from. A department is an org, so this reads the same at every level, and a value set here is what the
 * orgs, projects and topics below it start from unless a lock above says otherwise.
 */
import { useTranslation } from 'react-i18next';

import { SettingsEditor } from '@/features/settings/Editor';
import type { RowPlace } from '@/features/settings/SettingRow';
import type { Org } from '@/generated/api-types';
import { Section } from '@/shell/Page';

/** What a row needs to know of the org it is shown at. */
export function placeOf(org: Pick<Org, 'name' | 'owner' | 'access' | 'kind'>): RowPlace {
  return { name: org.name, owner: org.owner, access: org.access, kind: org.kind };
}

export function Policies({ org }: { org: Org }) {
  const { t } = useTranslation();
  return (
    <Section id="policies" title={t('day100.names.defaults')}>
      <SettingsEditor target={{ kind: 'node', id: org.id }} scope="org" place={placeOf(org)} secrets />
    </Section>
  );
}
