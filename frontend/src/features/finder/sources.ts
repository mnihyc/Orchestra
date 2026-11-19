/** Where each group of the finder gets its rows: the names the server finds, what the page holds, and what the reader starred.
 *
 * The local groups answer for every key. The named things are asked for 150 ms after the last key: the
 * lookup is keyed by the letters that have rested, so that a word typed quickly is one question, and the
 * answer to the letters before stays on screen until the next arrives. Each group of names is asked for
 * on its own, so that the many resources of an org can never crowd out the one topic that was meant.
 */
import { openPage } from '@/features/workbench/PageView';
import { useQuery } from '@tanstack/react-query';
import { useRouter } from '@tanstack/react-router';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { orgsQuery } from '@/api/nodes';
import { permissionsQuery } from '@/api/access';
import { ALL_NAME_KINDS, namesQuery, PEOPLE_KINDS, PLACE_KINDS, RESOURCE_KINDS, settingRegistryQuery } from '@/api/search';
import { useFinder } from '@/features/finder/store';
import { settingsQuery } from '@/api/settings';
import { type OpenHow, useCommands } from '@/features/finder/commands';
import { type Destination, pathOf } from '@/features/finder/destinations';
import {
  commandEntries,
  commandRows,
  helpEntries,
  helpCommands,
  helpRows,
  nearestPlace,
  offerRows,
  permissionEntries,
  permissionRows,
  settingDestination,
  settingEntries,
  settingRows,
} from '@/features/finder/local';
import type { Mode } from '@/features/finder/modes';
import { matchScore } from '@/features/finder/ranking';
import { chatWorthARow, type Env, type GroupId, rowOfName, type Scored } from '@/features/finder/rows';
import { useRowOpener } from '@/features/finder/pageRows';
import { useHelpOpener } from '@/features/help/open';
import { localizedSetting } from '@/features/settings/settingText';
import { openResult, targetOfDestination } from '@/features/search/ResultView';
import { labelsOfNames } from '@/features/search/labels';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import type { NameHit } from '@/generated/api-types';
import { useLocalizedGuide as useGuide } from '@/help/localized';
import { useDebounced } from '@/lib/debounce';
import { useNodePath, useTreeStore } from '@/realtime/stores/tree';
import { useSelectedOrg } from '@/shell/org';
import { useCurrentHeading } from '@/shell/screen';

/** How long the field rests before the server is asked: long enough that the keys of one word are one question. */
export const NAMES_PAUSE_MS = 150;

/** How many names of each group are asked for: the contract's default. */
const NAMES_ASKED = 20;
/** How many recents the empty field lists. */
const RECENTS_ASKED = 8;

/** The finder's way of going somewhere, and of knowing whether somewhere is there. */
export function useEnv(showCard: (hit: NameHit) => void, ready = true): Env {
  const { t } = useTranslation();
  const router = useRouter();
  const orgId = useSelectedOrg((state) => state.orgId);
  const orgs = useQuery({ ...orgsQuery, enabled: ready }).data;
  const orgSlug = orgs?.find((org) => org.id === orgId)?.slug ?? null;
  const knownPlaces = useKnownPlaces();
  const heading = useCurrentHeading();
  const { path: here } = useNodePath(heading?.nodeId);
  const project = here.findLast((node) => node.kind === 'project');
  return useMemo<Env>(
    () => ({
      t,
      orgSlug,
      knownPlaces,
      projectId: project?.id ?? null,
      projectPath: project === undefined ? [] : here.map((node) => node.id),
      exists: (to) => Object.hasOwn(router.routesByPath, to),
      showCard,
      go: (destination: Destination, how: OpenHow) => {
        const path = pathOf(destination);
        if (how === 'beside') {
          const target = targetOfDestination(destination);
          if (target !== null) { openResult(target, destination); return; }
        }
        if (how === 'tab') { openPage(path, 'new'); }
        else router.history.push(path);
      },
    }),
    [t, router, orgSlug, showCard, project, here, knownPlaces],
  );
}

/** What a group of names is asked for, and whether it is asked for at all. */
function useNames(wanted: boolean, query: string, kinds: readonly NameHit['kind'][], limit: number, starred = false) {
  const participating = useFinder((state) => state.participating);
  // The recents are asked for at every opening: where the reader went a moment ago is what they look for.
  const search = useQuery({ ...namesQuery({ q: query, kinds, limit, participating, starred }), select: (page) => page, enabled: wanted, ...(query === '' ? { staleTime: 0 } : {}) });
  return { hits: wanted ? search.data?.items : undefined, partial: wanted && search.data?.partial === true, fetching: wanted && search.isFetching, error: wanted && search.isError ? search.error : null };
}

export interface Answers {
  answers: Partial<Record<GroupId, readonly Scored[]>>;
  /** Remote groups at their answer limit or access-check budget need a complete-list link. */
  full: ReadonlySet<GroupId>;
  /** Names are still being waited for: the list is not yet all there. */
  pending: boolean;
  /** The bounded lookup may have more names; narrower words can reach them. */
  partial: boolean;
  /** The lookup of names failed, and how. */
  failure: unknown;
}

export interface AnswerInput {
  commandPrefix?: string | null;
  /** Whether the finder is open: nothing is asked of the server while it is not. */
  active: boolean;
  /** Shell org metadata comes from bootstrap; explicit name searches may run while it is pending. */
  metadataReady?: boolean;
  mode: Mode;
  /** What is typed, without the sign. */
  query: string;
  env: Env;
  /** Opens a conversation search for the words, given to the row that stands for it. */
  searchRow: Scored | null;
  helpForScreen: () => void;
  commandsHere: () => void;
}

/** Every group's rows for what is typed. */
export function useAnswers({ active, mode, query, env, searchRow, helpForScreen, commandsHere, commandPrefix, metadataReady = true }: AnswerInput): Answers {
  const { t } = env;
  const { i18n } = useTranslation();
  const typed = query !== '';
  const rested = useDebounced(query, NAMES_PAUSE_MS);
  const waiting = rested !== query;

  const everywhere = mode === 'all';
  const wantGo = active && everywhere && typed;
  const wantPeople = active && typed && (everywhere || mode === 'people');
  const wantResources = active && everywhere && typed;
  const wantRecent = active && everywhere && !typed;

  const go = useNames(wantGo, rested, PLACE_KINDS, NAMES_ASKED);
  const people = useNames(wantPeople, rested, PEOPLE_KINDS, NAMES_ASKED);
  const resources = useNames(wantResources, rested, RESOURCE_KINDS, NAMES_ASKED);
  const recent = useNames(wantRecent, '', ALL_NAME_KINDS, RECENTS_ASKED);
  const favorites = useNames(wantRecent, '', ALL_NAME_KINDS, NAMES_ASKED, true);

  // What the page holds.
  const commands = useCommands();

  const wantSettings = active && typed && (everywhere || mode === 'settings');
  const registry = useQuery({ ...settingRegistryQuery, enabled: wantSettings }).data;
  const catalogue = useQuery({ ...permissionsQuery, enabled: wantSettings }).data;
  // A setting is found by its name and words in the reader's language, as the settings editor shows it.
  const language = i18n.resolvedLanguage;
  const settingList = useMemo(() => settingEntries((registry ?? []).map((def) => localizedSetting(def, i18n, language))), [registry, i18n, language]);
  const openRow = useRowOpener();
  const permissionList = useMemo(() => {
    if (openRow === null) return [];
    const supported = openRow.permissions;
    return permissionEntries((catalogue?.permissions ?? []).filter((permission) => supported === null || supported.has(permission.name)));
  }, [catalogue,openRow]);

  const heading = useCurrentHeading();
  const { path } = useNodePath(heading?.nodeId);
  const orgId = useSelectedOrg((state) => state.orgId);
  const place = path.at(-1)?.id ?? orgId;
  // A screen that is of no place is in the org in view.
  const orgPath = useMemo(() => (orgId === null ? [] : [{ id: orgId, kind: 'org' as const }]), [orgId]);
  const resolved = useQuery({ ...settingsQuery(place ?? ''), enabled: wantSettings && place !== null, retry: false }).data;
  const resolvedByKey = useMemo(() => new Map((resolved ?? []).map((entry) => [entry.key, entry])), [resolved]);
  const orgs = useQuery({ ...orgsQuery, enabled: active && metadataReady }).data;

  const unscoped = commandPrefix === null || commandPrefix === undefined;
  const wantHelp = active && (typed && (everywhere || mode === 'help') || mode === 'commands' && unscoped);
  const guide = useGuide(wantHelp);
  const helpList = useMemo(() => helpEntries(guide?.articles ?? []), [guide]);
  const openHelp = useHelpOpener();
  const commandList = active ? commandEntries(unscoped ? (mode === 'commands' ? [...commands, ...helpCommands(guide?.articles ?? [], openHelp)] : commands)
    : commands.filter((command) => command.id.startsWith(commandPrefix)), t) : [];

  const answers: Partial<Record<GroupId, readonly Scored[]>> = {};
  const favoriteNames = favorites.hits ?? [];
  const namedEnv: Env = { ...env, placeLabels: labelsOfNames([...favoriteNames, ...(recent.hits ?? []), ...(go.hits ?? []), ...(people.hits ?? []), ...(resources.hits ?? [])], env.knownPlaces) };

  if (everywhere && !typed) {
    answers.actions = offerRows(env, helpForScreen, commandsHere);
    const hits = favoriteNames;
    answers.starred = hits.map((hit) => ({ row: rowOfName(hit, 'starred', '', namedEnv, true), score: 0 }));
    const seen = new Set(hits.map((hit) => `${hit.kind}:${hit.id}`));
    answers.recent = (recent.hits ?? []).filter((hit) => !seen.has(`${hit.kind}:${hit.id}`)).map((hit) => ({ row: rowOfName(hit, 'recent', '', namedEnv), score: 0 }));
  }

  if (wantGo && go.hits !== undefined) answers.go = withStars(go.hits.filter((hit) => chatWorthARow(hit, saidIn)), 'go', rested, namedEnv);
  if (wantPeople && people.hits !== undefined) answers.people = people.hits.map((hit) => ({ row: rowOfName(hit, 'people', rested, namedEnv), score: matchScore(hit.name, rested) }));
  if (wantResources && resources.hits !== undefined) answers.resources = resources.hits.map((hit) => ({ row: rowOfName(hit, 'resources', rested, namedEnv), score: matchScore(hit.name, rested) }));

  // With nothing typed the finder offers recents and what the screen can do; the whole list of commands is for `>`.
  if (active && ((mode === 'all' && typed) || mode === 'commands')) {
    const rows = commandRows(commandList, query, env);
    if (rows.length > 0) answers.commands = rows;
  }
  if (wantSettings && registry !== undefined) {
    const rootSlug = (id: string) => orgs?.find((org) => org.id === id)?.address
      ?? [...useTreeStore.getState().byOrg.values()].map((tree) => tree.nodes.get(id)).find((node) => node?.kind === 'org')?.address ?? id;
    const rows = settingRows(settingList, query, env, {
      resolved: resolvedByKey,
      place: (def) => settingDestination(def, nearestPlace(def, path.length > 0 ? path : orgPath), rootSlug),
    });
    if (rows.length > 0) answers.settings = rows;
  }
  if (wantSettings && catalogue !== undefined && openRow !== null) {
    const rows = permissionRows(permissionList, query, openRow);
    if (rows.length > 0) answers.rows = rows;
  }
  if (wantHelp && guide !== undefined) {
    const rows = helpRows(helpList, query, env, openHelp);
    if (rows.length > 0) answers.help = rows;
  }
  if (searchRow !== null) answers.conversations = [searchRow];

  const pending = (wantGo || wantPeople || wantResources) && (waiting || go.fetching || people.fetching || resources.fetching);
  const full = new Set<GroupId>();
  for (const [id, answer, limit] of [
    ['go', go, NAMES_ASKED], ['people', people, NAMES_ASKED], ['resources', resources, NAMES_ASKED],
    ['recent', recent, RECENTS_ASKED], ['starred', favorites, NAMES_ASKED],
  ] as const) if (answer.partial || (answer.hits?.length ?? 0) >= limit) full.add(id);
  return { answers, full, partial: go.partial || people.partial || resources.partial || recent.partial || favorites.partial, pending: pending || (wantRecent && (recent.fetching || favorites.fetching)), failure: go.error ?? people.error ?? resources.error ?? recent.error ?? favorites.error };
}

/** The names of "Go to", with what the reader starred first and then the order the server gave them in. */
/** Whether anything was said in a place's chat, as far as the trees held know; `null` where no tree held holds the place. */
function saidIn(nodeId: string): boolean | null {
  for (const tree of useTreeStore.getState().byOrg.values()) {
    const node = tree.nodes.get(nodeId);
    if (node !== undefined) return node.chat.seq > 0;
  }
  return null;
}

function withStars(hits: readonly NameHit[], group: GroupId, query: string, env: Env): Scored[] {
  const scored = hits.map((hit, index) => ({ hit, index, score: matchScore(hit.name, query), star: hit.starred === true }));
  // Name relevance comes first; ties keep the server's personal-visit and activity order.
  scored.sort((one, other) => other.score - one.score || one.index - other.index);
  return scored.map((entry) => ({ row: rowOfName(entry.hit, group, query, env, entry.star), score: entry.score }));
}
