/** Roles and defaults of an org: who holds a role in it and above it, and the defaults it gives to what is made inside it.
 *
 * Both are the permission page's own, not drawn a second time: the People of the place stand here as that page
 * draws them, with what it lets a person do to a role, and the Defaults open on the page itself, where they are
 * read and changed.
 */
import { SlidersHorizontal } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { AccessTarget } from '@/api/access';
import { openAccess } from '@/features/access/AccessDialog';
import { Roles } from '@/features/access/Roles';
import type { Org } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { actionName } from '@/ui/actions';
import { Button } from '@/ui/Button';

export function RolesAndDefaults({ org }: { org: Org }) {
  const { t } = useTranslation();
  const place: AccessTarget = { kind: 'org', id: org.id, name: org.name };
  const openDefaults = () => openAccess(place, { tab: 'defaults', share: false });
  return (
    <Section
      id="roles-held"
      title={t('day100.names.roles')}
      action={
        <Button icon={<SlidersHorizontal aria-hidden />} onClick={openDefaults}>
          {actionName('defaults')}
        </Button>
      }
    >
      <Roles place={place} />
    </Section>
  );
}
