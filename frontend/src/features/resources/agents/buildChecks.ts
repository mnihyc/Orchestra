/** Compatibility results describe the observation and its consequence in the reader's words. */
import type { TFunction } from 'i18next';
import type { HarnessBuild } from '@/generated/api-types';
import type { TranslationKey } from '@/i18n';
import { localizedText } from '@/features/resources/executorNames';

type BuildCase = NonNullable<HarnessBuild['check']>['cases'][number];

const CASE_NAMES: Readonly<Record<string, TranslationKey | undefined>> = {
  runner: 'programVersions.checks.runner',
  startup: 'programVersions.checks.startup',
  command_approval: 'programVersions.checks.commandApproval',
  user_input: 'programVersions.checks.userInput',
  elicitation: 'programVersions.checks.elicitation',
  nested_subagent: 'programVersions.checks.subagent',
  native_import: 'programVersions.checks.nativeImport',
  'history.native_import': 'programVersions.checks.nativeImport',
  'tools.result_form': 'programVersions.checks.toolResults',
  parallel_calls: 'programVersions.checks.parallelCalls',
  image_result: 'programVersions.checks.imageResult',
  sandbox: 'programVersions.checks.sandbox',
};

const FEATURE_NAMES: Readonly<Record<string, TranslationKey | undefined>> = {
  history_delete: 'agentHistory.nativeDelete',
  history_delete_fallback: 'agentHistory.safeFallback',
  'approvals.command': 'programVersions.disabledFeatures.commandApproval',
  'approvals.user_input': 'programVersions.disabledFeatures.questions',
  'approvals.mcp_elicitation': 'programVersions.disabledFeatures.toolQuestions',
  subagents: 'programVersions.disabledFeatures.subagents',
  plan_mode: 'programVersions.disabledFeatures.planMode',
  orch_tools: 'programVersions.disabledFeatures.sharedTools',
  input_images: 'programVersions.disabledFeatures.images',
  steer: 'programVersions.disabledFeatures.steering',
  fork_native: 'programVersions.disabledFeatures.nativeFork',
  import_native: 'programVersions.disabledFeatures.nativeImport',
  import_neutral: 'programVersions.disabledFeatures.neutralImport',
  discover_sessions: 'programVersions.disabledFeatures.sessionDiscovery',
  model_discovery: 'programVersions.disabledFeatures.modelDiscovery',
  background_commands: 'programVersions.disabledFeatures.backgroundCommands',
  reasoning_summary: 'programVersions.disabledFeatures.reasoning',
  plan: 'programVersions.disabledFeatures.planUpdates',
};

/** Read the program catalogue's localized title without displaying an identifier as a title. */
function catalogueTitle(catalogue: unknown, collection: 'cases' | 'features', key: string, locale: string): string | null {
  if (typeof catalogue !== 'object' || catalogue === null || !(collection in catalogue)) return null;
  const entries = (catalogue as Record<string, unknown>)[collection];
  if (!Array.isArray(entries)) return null;
  const entry: unknown = entries.find((value: unknown) => typeof value === 'object' && value !== null
    && (collection === 'cases' ? 'name' in value && value.name === key : 'id' in value && value.id === key));
  return typeof entry === 'object' && entry !== null && 'title' in entry ? localizedText(entry.title, locale) : null;
}

/** A catalogue titles cases; an older reply without one retains its existing diagnostic. */
export function checkName(name: string, t: TFunction, catalogue?: unknown, locale = 'en'): { text: string; known: boolean } {
  if (catalogue !== undefined && catalogue !== null) {
    const runner = name === 'runner' && typeof catalogue === 'object' && 'runner' in catalogue ? localizedText(catalogue.runner, locale) : null;
    return { text: runner ?? catalogueTitle(catalogue, 'cases', name, locale) ?? t('admin.harnesses.checkResults'), known: true };
  }
  const key = Object.hasOwn(CASE_NAMES, name) ? CASE_NAMES[name] : undefined;
  return key === undefined ? { text: name, known: false } : { text: t(key), known: true };
}

/** A newer checker may distinguish an unsupported archive from a failed observation. */
export function unsupportedReason(result: BuildCase): string | null {
  return 'unsupported' in result && typeof result.unsupported === 'string' ? result.unsupported : null;
}

export function caseUnavailable(result: BuildCase): boolean {
  return !result.passed || unsupportedReason(result) !== null;
}

/**
 * The cases of the program's present manifest that the build's stored check has not run, by their titles: not failed and not
 * unverified, but pending. Empty without a catalogue to say which cases there are, and for a build that was never checked.
 */
export function pendingCases(build: HarnessBuild, catalogue: unknown, locale: string): string[] {
  if (build.check === undefined || build.check === null || typeof catalogue !== 'object' || catalogue === null || !('cases' in catalogue)) return [];
  const entries = (catalogue as Record<string, unknown>).cases;
  if (!Array.isArray(entries)) return [];
  const ran = new Set(build.check.cases.map((result) => result.name));
  return entries.flatMap((entry: unknown) => {
    if (typeof entry !== 'object' || entry === null || !('name' in entry) || typeof entry.name !== 'string' || ran.has(entry.name)) return [];
    return [('title' in entry ? localizedText(entry.title, locale) : null) ?? entry.name];
  });
}

/** Feature protocol identifiers remain data even when an external adapter adds a new one. */
export function featureName(feature: string, t: TFunction, catalogue?: unknown, locale = 'en'): string {
  if (catalogue !== undefined && catalogue !== null) return catalogueTitle(catalogue, 'features', feature, locale) ?? t('programVersions.disabledFeatures.other');
  const key = Object.hasOwn(FEATURE_NAMES, feature) ? FEATURE_NAMES[feature] : undefined;
  return t(key ?? 'programVersions.disabledFeatures.other');
}

export function disabledFeatures(build: HarnessBuild): string[] {
  return [...new Set([...(build.check?.off ?? []), ...(build.check?.cases ?? []).flatMap((result) => caseUnavailable(result) && result.feature !== null ? [result.feature] : [])])];
}

/** How far the weakest of several builds is known to work: the one a person is told of when more than one build of a version could be the one held. */
const WEAKEST_FIRST: readonly HarnessBuild['status'][] = ['failed', 'checking', 'untested', 'checked'];

/**
 * The stored build of exactly this kind and version, as Orch's check left it. A version has a build for each system and processor; the
 * machine that reported it names its system and not its processor, so of the builds for its system (any, when none is for it) the one
 * that is known to work least is shown: a check that failed is not left unsaid because a sibling passed. `null` when no build of it is known.
 */
export function storedBuild(builds: readonly HarnessBuild[] | undefined, kind: string, version: string, os?: HarnessBuild['os']): HarnessBuild | null {
  const same = (builds ?? []).filter((build) => build.kind === kind && build.version === version);
  const there = same.filter((build) => build.os === os);
  const candidates = there.length > 0 ? there : same;
  return [...candidates].sort((one, other) => WEAKEST_FIRST.indexOf(one.status) - WEAKEST_FIRST.indexOf(other.status))[0] ?? null;
}

/** The server reports the history fallback for this build after its checks. */
export function featureFallback(build: HarnessBuild, feature: string, t: TFunction): string | null {
  const carry = 'carry' in build ? build.carry : undefined;
  if (feature === 'import_native') {
    if (carry === 'items') return t('programVersions.fallback.neutralCarry');
    if (carry === 'handoff') return t('programVersions.fallback.handoff');
    return null;
  }
  if (feature === 'import_neutral' && carry === 'handoff') return t('programVersions.fallback.handoffWithoutNative');
  return null;
}
