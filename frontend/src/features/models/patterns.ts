/** How a pattern names models, and the name of a group: the one rule by which a cap, an allowlist or an entry says which models it covers, read the way the server reads it.
 *
 * A pattern is compared with a model's routing name, both in lower case. `*` stands for any run of
 * characters, and a pattern without one names one model. A pattern that holds no `/` is compared with what
 * follows the last `/` of the name, so that `gpt-6*` covers `gpt-6-sol` and `openai/gpt-6-sol` alike; one that
 * holds a `/` is compared with the whole name. `@<group>` names every model that its provider's managers put
 * in that group.
 */

/** How long the name of a group may be, as the contract writes it. */
export const GROUP_NAME_LONGEST = 40;
/** In how many groups one model may be, as the contract writes it. */
export const GROUPS_MOST = 8;

/** A group is named by letters a to z, digits, `-` and `_`, beginning with a letter or a digit. */
const GROUP_NAME = new RegExp(`^[a-z0-9][a-z0-9_-]{0,${GROUP_NAME_LONGEST - 1}}$`);

/** Whether a text is a name a group may have. */
export function isGroupName(name: string): boolean {
  return GROUP_NAME.test(name);
}

/** Whether a pattern is written as the name of a group. */
export function isGroupPattern(pattern: string): boolean {
  return pattern.startsWith('@');
}

/** The pattern that names a group. */
export const groupPattern = (group: string): string => `@${group}`;

const escapeForExpression = (text: string): string => text.replace(/[.+?^${}()|[\]\\]/g, '\\$&');

/** Whether a name fits a pattern with `*` in it. */
function fits(pattern: string, name: string): boolean {
  if (!pattern.includes('*')) return pattern === name;
  const expression = pattern.split('*').map(escapeForExpression).join('.*');
  return new RegExp(`^${expression}$`).test(name);
}

/** Whether a pattern of a list that says which models something covers names a model, given the groups its entry at the provider is in. */
export function patternNames(pattern: string, routingName: string, groups: readonly string[]): boolean {
  const wanted = pattern.trim().toLowerCase();
  if (wanted === '') return false;
  if (isGroupPattern(wanted)) return groups.includes(wanted.slice(1));
  const name = routingName.toLowerCase();
  const compared = wanted.includes('/') ? name : name.slice(name.lastIndexOf('/') + 1);
  return fits(wanted, compared);
}

/** Whether any pattern of a list names the model. */
export function listNames(patterns: readonly string[], routingName: string, groups: readonly string[]): boolean {
  return patterns.some((pattern) => patternNames(pattern, routingName, groups));
}
