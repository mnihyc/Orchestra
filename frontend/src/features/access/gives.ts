/** What the reader of a page may give: the choices and the roles that are theirs to name, the words that say why the rest is not, and the case of a place that keeps sharing to owners.
 *
 * Nobody gives more than they hold, so a form offers what the page says its reader may give
 * (`PermissionPage.gives`, `PermissionRow.can_give`) and shows the rest switched off with one line that
 * says why in the reader's own terms: the role they hold in a place, or how far they hold a thing. Nothing
 * here asks the server: it reads the page the form was drawn from.
 */
import type { TFunction } from 'i18next';

import type { AccessChoice, PermissionPage, Role } from '@/generated/api-types';

import { CHOICE_ORDER, middleOf, ROLE_ORDER } from './model';

/** The role that goes with a choice when it is given on a place with what is in it: the one whose work the choice names. Admin is given among the place's People. */
export const ROLE_OF_CHOICE: Record<AccessChoice, Role> = { full_view: 'viewer', full_interact: 'interactor', full_manage: 'developer' };

/** The choice a role's work comes to: Viewers watch, Interactors take part, Developers and Admins manage. */
export function choiceOfRole(role: Role): AccessChoice {
  return role === 'viewer' ? 'full_view' : role === 'interactor' ? 'full_interact' : 'full_manage';
}

/** Whether the reader may give a choice here. */
export const mayGiveChoice = (page: PermissionPage, choice: AccessChoice): boolean => page.gives.choices.includes(choice);

/** Whether the reader may give a choice as a role in a place: the role that goes with it, or a higher one, is theirs to give. */
export const mayGiveAsRole = (page: PermissionPage, choice: AccessChoice): boolean => page.gives.roles.includes(ROLE_OF_CHOICE[choice]);

/** The highest choice the reader may give, or `null` where they may give none. */
export function highestChoice(page: PermissionPage): AccessChoice | null {
  return [...CHOICE_ORDER].reverse().find((choice) => page.gives.choices.includes(choice)) ?? null;
}

/** The first choice the reader may give, lowest first: where a form begins when the one it would begin on is not theirs. */
export function firstChoice(page: PermissionPage, wanted: AccessChoice): AccessChoice {
  return page.gives.choices.includes(wanted) ? wanted : (CHOICE_ORDER.find((choice) => page.gives.choices.includes(choice)) ?? wanted);
}

/** The highest role the reader may give in a place, or `null` where they may give none. */
export function highestRole(page: PermissionPage): Role | null {
  return [...ROLE_ORDER].reverse().find((role) => page.gives.roles.includes(role)) ?? null;
}

/** Whether the reader may give nothing at all here, whichever the way: no choice, no role, nothing to agents. */
export const givesNothing = (page: PermissionPage): boolean => page.gives.choices.length === 0 && page.gives.roles.length === 0 && page.gives.agents.length === 0;

/** Whether the place keeps sharing to a thing's owners and to whoever manages its page, and this reader is neither: the field is theirs only to ask about. */
export const sharingIsKept = (page: PermissionPage): boolean => !page.gives.holders_share && givesNothing(page);

export interface GiveWords {
  t: TFunction;
  /** What the page is of. */
  name: string;
  /** The kind of thing, as the catalogue words it: `thread`, `topic`, `provider`. */
  kind: string;
  /** What a choice is called for this kind of thing. */
  choice: (choice: AccessChoice) => string;
}

/**
 * The one line that says why the rest of what could be given is not the reader's to give, in their own
 * terms; `null` where all of it is theirs. On a place the reader is told the role they hold and how far it
 * goes; on anything else, how far they hold the thing.
 */
export function whyNot(page: PermissionPage, words: GiveWords, options: { onPlace: boolean }): string | null {
  const { t, name, kind, choice } = words;
  const best = highestChoice(page);
  const role = highestRole(page);
  if (options.onPlace && role !== null) {
    if (role === 'admin') return null;
    const above = ROLE_ORDER[ROLE_ORDER.indexOf(role) + 1] ?? 'admin';
    return t(`access.gives.youAre.${role}`, { place: name, choice: choice(choiceOfRole(role)), above: t(`roles.${above}`) });
  }
  if (best === null || best === 'full_manage') return null;
  const verb = t(`access.gives.verb.${middleOf(kind)}`);
  return best === 'full_view' ? t('access.gives.readOnly', { name, verb }) : t('access.gives.upTo', { name, choice: choice(best) });
}

/** The words that say a place keeps sharing to its owners and who they are: "Sharing in Acme is with a thing’s owners: Alice." */
export function keptLine(t: TFunction, page: PermissionPage, place: string): string {
  const owners = page.owners.map((entry) => (entry.holder.kind === 'user' ? entry.holder.user.display_name : entry.holder.kind === 'group' ? entry.holder.group.name : '')).filter((text) => text !== '');
  return owners.length === 0 ? t('access.gives.keptNobody', { place }) : t('access.gives.kept', { place, owners: owners.join(', ') });
}
