/** The rules an org gives its invitations, said beside the Invite button and not only inside its dialog, with where each comes from.
 *
 * The dialog they are the rules of is `features/org/Invite.tsx`: the two forms an invitation has, only those the root
 * org allows, a life that starts at the org's default and goes no further than its most, and the link shown once.
 * Said here, nobody has to open the dialog to learn what it will refuse.
 */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { settingsQuery } from '@/api/settings';
import { durationLabel, invitationRules } from '@/features/org/Invite';
import type { Org, ResolvedSetting } from '@/generated/api-types';

/** The keys of the rules an org gives its invitations. */
const RULE_KEYS = ['invitations.forms', 'invitations.default_hours', 'invitations.max_hours', 'invitations.max_uses'] as const;

/** The names of the places the rules come from, each once, without the shipped defaults. */
export function ruleSources(settings: readonly ResolvedSetting[] | undefined): string[] {
  const names = new Set<string>();
  for (const key of RULE_KEYS) {
    const found = settings?.find((setting) => setting.key === key);
    const name = found?.source.node?.name;
    if (name !== undefined && name !== null) names.add(name);
  }
  return [...names];
}

/** The rules in force for the invitations of an org, in a sentence or two. */
export function InvitationRules({ org }: { org: Org }) {
  const { t } = useTranslation();
  const settings = useQuery(settingsQuery(org.id));
  if (settings.data === undefined) return null;
  const rules = invitationRules(settings.data);
  const sources = ruleSources(settings.data);
  const forms = rules.forms.map((form) => t(`places.invite.forms.${form}`));
  return (
    <div className="flex flex-col gap-1 text-sm text-text-muted">
      <p>{forms.length === 0 ? t('settings.members.rules.noForms') : t('settings.members.rules.forms', { forms: forms.join(t('startFlow.separator')) })}</p>
      <p>
        {t('settings.members.rules.lives', {
          default: durationLabel(rules.defaultHours, t),
          most: durationLabel(rules.maxHours, t),
          uses: rules.maxUses,
        })}
      </p>
      <p>{sources.length === 0 ? t('settings.members.rules.shipped') : t('settings.members.rules.setBy', { places: sources.join(', ') })}</p>
    </div>
  );
}
