/** What a plan says, in words: the three short lists (what changes, what stays, what ends), how the history travels, how long it takes.
 *
 * A plan names things and numbers; the person reads sentences, and the same sentences are read before a
 * confirmation and after the job is done. This is where each member of a plan is turned into one line, with
 * the tone it deserves: a plain line, a warning where something is opened or lost, a danger where it cannot go
 * on. Nothing here asks the server for anything or draws anything.
 */
import type { TFunction } from 'i18next';
import type { MeasurementSizeData } from '@/features/change/Measurements';

import type { EnvironmentRef, FilesPlan, ForkPlan, HistoryMethod, Holder, NodeRef, PendingUpdate, PlanBlocker, PlanMcpServer, PlanMount, PlannedEnvironment, ProviderModel, UserRef } from '@/generated/api-types';
import { effortLabel, modelLabel } from '@/features/change/modelLabel';
import { formatBytes, formatNumber } from '@/lib/format';
import { permissionLabel } from '@/lib/permissions';
import { foldName } from '@/lib/placeLabel';
import type { NamedResource } from '@/features/resource-facts/ResourceSentence';
import { formatSpanRange, SECOND_MS } from '@/lib/time';

export type Tone = 'plain' | 'warning' | 'danger';

/** One line of a list. `hint` names the choice it stands for, where a "?" belongs beside it. */
export interface Line {
  id: string;
  tone: Tone;
  text: string;
  resources?: readonly NamedResource[];
  /** Exact technical values stay behind the renderer's disclosure, outside the ordinary sentence. */
  technical?: PendingUpdate;
  size?: MeasurementSizeData;
  suffix?: string;
  hint?: 'history' | 'leaves' | 'page';
}

/** What a plan is read against: the thread it begins from, and who is reading. */
export interface PlanContext {
  kind: 'fork' | 'switch';
  thread: { name: string; model: string | null; modelDisplay: string | null; effort: string | null; owner: UserRef };
  me: string | null;
  locale: string;
  models?: readonly (Pick<ProviderModel, 'model_name' | 'display_name'> & Partial<Pick<ProviderModel, 'parameters'>>)[];
  /** Readable origins from the provider list this dialog already read, never from an address. */
  providerNames?: ReadonlyMap<string, string>;
}

const environmentName = (ref: EnvironmentRef | null): string => ref?.name ?? '';

/** A namesake is qualified by its visible origin; absent origins are described by the provider's role in this plan. */
export function planProviderName(plan: { from: { provider: { id: string; name: string } }; to: { provider: { id: string; name: string } } }, side: 'from' | 'to', context: Pick<PlanContext, 'providerNames'>, t: TFunction): string {
  const ref = plan[side].provider;
  const other = plan[side === 'from' ? 'to' : 'from'].provider;
  const name = context.providerNames?.get(ref.id) ?? ref.name;
  const otherName = context.providerNames?.get(other.id) ?? other.name;
  return ref.id !== other.id && foldName(name) === foldName(otherName)
    ? t(side === 'from' ? 'change.runs.providerCurrent' : 'change.runs.providerSelected', { name })
    : name;
}

/** An older preview has no approval choice; known choices use their catalogue names. */
export function plannedApprovalWords(to: object, t: TFunction): string | null {
  if (!('approval_mode' in to)) return null;
  const mode = to.approval_mode;
  if (mode !== 'ask' && mode !== 'on_request' && mode !== 'auto' && mode !== 'bypass') return null;
  return t('switchChoice.approvalMode', { mode: t(`choiceCatalog.approvalMode.${mode}.name`) });
}

/** The name of what the result runs in, whether it exists already or is to be made. */
export function targetName(to: ForkPlan['to']['environment']): string {
  return 'new' in to ? to.new.name : to.name;
}

/** Whether the result runs in an environment that is made for it. */
export function isNewEnvironment(to: ForkPlan['to']['environment']): to is { new: PlannedEnvironment } {
  return 'new' in to;
}

/** A model as it is read: its name, with the effort it is asked for. */
export function modelWords(model: string, effort: string | null): string {
  return effort === null ? model : `${model} (${effort})`;
}

/** A plan carries the routing value; its labels come from the existing provider-model read. */
export function targetModelName(plan: ForkPlan, context: PlanContext, t: TFunction): string {
  const current = plan.to.provider.id === plan.from.provider.id && plan.to.model === context.thread.model ? context.thread.modelDisplay : null;
  return modelLabel(plan.to.model, context.models ?? [], current, t);
}

export function targetModelWords(plan: ForkPlan, context: PlanContext, t: TFunction): string {
  return modelWords(targetModelName(plan, context, t), effortLabel(plan.to.effort, t));
}

/** The badge of how well a history was kept. */
export function fidelityKey(method: HistoryMethod, fidelity: ForkPlan['history']['report']['fidelity']): 'lossless' | 'structured' | 'summary' | 'none' {
  return method === 'none' ? 'none' : fidelity;
}

/** The words for a span of seconds as a range: "20–45 s", "6–9 min". */
export function rangeWords(low: number, high: number, locale: string): string {
  return formatSpanRange(low * SECOND_MS, high * SECOND_MS, { locale });
}

/** The names of the holders a list of them stands for, shortly: "Developers", "Ada Lovelace", "Everyone". */
export function holderNames(holders: readonly Holder[], t: TFunction): string {
  const names = holders.map((holder) => {
    switch (holder.kind) {
      case 'role':
        return t(`roles.${holder.role}`, { defaultValue: holder.role });
      case 'user':
        return holder.user.display_name;
      case 'group':
        return holder.group.name;
      case 'node':
      case 'agents':
        return holder.node.name ?? '';
      case 'everyone':
        return t('change.plan.everyone');
    }
  });
  return names.filter((name) => name !== '').join(', ');
}

// ---------------------------------------------------------------- the three lists

/** What the files of a plan come to, one line each: how many and how big, what cannot be copied, the home, what was changed in mounts. */
export const filesLines = (plan: FilesPlan, locale: string, t: TFunction): Line[] => {
  const lines:Line[]=plan.kept_image_bytes === undefined || plan.kept_image_bytes === null ? [] : [{id:'kept-image',tone:'plain',text:t('keptLayers.carried',{size:formatBytes(plan.kept_image_bytes,locale)})}];
  lines.push({
    id: 'files',
    tone: 'plain',
    text: t('change.plan.files', { count: plan.file_count, size: formatBytes(plan.total_bytes, locale), up: formatBytes(plan.bytes_to_upload, locale), down: formatBytes(plan.bytes_to_download, locale) }),
  });
  if (plan.missing.count > 0) lines.push({ id: 'files-missing', tone: 'warning', text: t('change.plan.filesMissing', { count: plan.missing.count }) });
  if (plan.source === 'cached') lines.push({ id: 'files-cached', tone: 'warning', text: t('change.plan.filesCached') });
  if (plan.home !== null) {
    const home = plan.home;
    lines.push(home.measurement !== undefined && home.measurement !== null
      ? { id: 'home', tone: 'plain', text: t('liveWork.home'), size: { reference: home.measurement, measured: home.measured, file_count: home.file_count, total_bytes: home.total_bytes } }
      : { id: 'home', tone: home.measured ? 'plain' : 'warning', text: home.measured ? t('change.plan.home', { size: formatBytes(home.total_bytes, locale) }) : t('mountPlan.homeUnmeasured') });
  }
  if (plan.file_states !== null) {
    const states = plan.file_states;
    lines.push(states.measurement !== undefined && states.measurement !== null
      ? { id: 'file-states', tone: 'plain', text: t('liveWork.file_states'), size: { reference: states.measurement, measured: states.complete, file_count: null, total_bytes: states.total_bytes } }
      : { id: 'file-states', tone: states.complete ? 'plain' : 'warning', text: states.complete
        ? t('mountPlan.fileStates', { count: states.roots, size: formatBytes(states.total_bytes, locale) }) : t('mountPlan.fileStatesUnmeasured') });
  }
  for (const view of plan.private_views) {
    lines.push({ id: `view-${view.mount.id}`, resources: [{ label: view.mount.name, subject: { kind: 'mount', mount: view.mount } }], tone: view.carried ? 'plain' : 'warning', text: view.carried
      ? t('mountPlan.privateCarried', { mount: view.mount.name })
      : t('mountPlan.privateLeft', { mount: view.mount.name, reason: t(`mountPlan.privateReasons.${view.left_because ?? 'asked'}`) }) });
    lines.push(view.measurement !== undefined && view.measurement !== null
      ? { id: `view-size-${view.mount.id}`, resources: [{ label: view.mount.name, subject: { kind: 'mount', mount: view.mount } }], tone: 'plain', text: t('liveWork.privateCount', { name: view.mount.name }), size: { reference: view.measurement, measured: view.measured, file_count: view.file_count, total_bytes: view.total_bytes } }
      : { id: `view-size-${view.mount.id}`, resources: [{ label: view.mount.name, subject: { kind: 'mount', mount: view.mount } }], tone: view.measured ? 'plain' : 'warning', text: view.measured
        ? t('mountPlan.privateSize', { mount: view.mount.name, count: view.file_count, size: formatBytes(view.total_bytes, locale) })
        : t('mountPlan.privateUnmeasured', { mount: view.mount.name }) });
  }
  return lines;
};

/** Every mount's fate, including its target mode and the reason for a detachment. */
export function mountLines(mounts: readonly PlanMount[], t: TFunction): Line[] {
  return mounts.map((mount) => ({
    id: `mount-${mount.mount.id}`, tone: mount.action === 'detach' ? 'warning' : 'plain',
    resources: [{ label: mount.mount.name, subject: { kind: 'mount', mount: mount.mount } }],
    text: mount.action === 'detach'
      ? t('mountPlan.detached', { mount: mount.mount.name, reason: t(`mountPlan.detachReasons.${mount.detached_because ?? 'no_location'}`) })
      : t(`mountPlan.${mount.action}`, { mount: mount.mount.name, path: mount.path ?? '', mode: t(`mountPlan.modes.${mount.mode ?? 'read_only'}`) }),
  }));
}

/** What each copy of a mount's data carries and where it lands, one line each. */
export function copyLines(mounts: readonly PlanMount[], locale: string, t: TFunction): Line[] {
  return mounts.flatMap((mount): Line[] => {
    const copy = mount.copy;
    if (mount.fate !== 'copy' || copy === null) return [];
    const size = copy.measured ? t('mountPlan.copy.size', { count: copy.file_count, formattedCount: formatNumber(copy.file_count, locale), size: formatBytes(copy.total_bytes, locale) }) : '–';
    const where = copy.location.made ? t('mountPlan.copy.made', { name: copy.location.source ?? '' }) : t('mountPlan.copy.there');
    const resources: NamedResource[] = [{ label: mount.mount.name, subject: { kind: 'mount', mount: mount.mount } }];
    const lines: Line[] = [copy.measurement !== undefined && copy.measurement !== null
      ? { id: `copy-${mount.mount.id}`, resources, tone: 'plain', text: t('liveWork.copyMount', { name: mount.mount.name }), size: { reference: copy.measurement, measured: copy.measured, file_count: copy.file_count, total_bytes: copy.total_bytes }, suffix: where }
      : { id: `copy-${mount.mount.id}`, resources, tone: copy.measured ? 'plain' : 'warning', text: t('mountPlan.copy.line', { mount: mount.mount.name, size, where }) }];
    const holds = copy.holds;
    if (holds !== null) {
      const held = holds.measured ? t('mountPlan.copy.size', { count: holds.file_count, formattedCount: formatNumber(holds.file_count, locale), size: formatBytes(holds.total_bytes, locale) }) : '–';
      lines.push(holds.measurement !== undefined && holds.measurement !== null
        ? { id: `copy-kept-${mount.mount.id}`, resources, tone: copy.into === null && holds.measured ? 'warning' : 'plain', text: t('liveWork.targetCount', { name: mount.mount.name }), size: { reference: holds.measurement, measured: holds.measured, file_count: holds.file_count, total_bytes: holds.total_bytes }, suffix: t(copy.into === null ? 'liveWork.chooseCopy' : 'liveWork.keptCopy') }
        : { id: `copy-kept-${mount.mount.id}`, resources, tone: copy.into === null ? 'warning' : 'plain', text: copy.into === null ? t('mountPlan.copy.holds', { mount: mount.mount.name, size: held }) : t('mountPlan.copy.kept', { size: held }) });
    }
    return lines;
  });
}

/** What becomes of the MCP servers that a plan names as new, lost or unavailable there. */
export function mcpLines(servers: readonly PlanMcpServer[], t: TFunction): Line[] {
  return servers.map((server): Line => ({
    id: `mcp-${server.id}`,
    resources: [{ label: server.name, subject: { kind: 'mcp_server', server: { id: server.id, name: server.name } } }],
    tone: server.fate === 'new' || server.fate === 'kept' ? 'plain' : 'warning',
    text: server.reason === null ? t(`mcpPlan.${server.fate}`, { name: server.name }) : t(`mcpPlan.${server.fate}Because`, { name: server.name, reason: server.reason }),
  }));
}

/** What the result will be, and what is different: for a fork, first that it is its maker's. */
export function changesOf(plan: ForkPlan, context: PlanContext, t: TFunction): Line[] {
  const lines: Line[] = [];
  const fork = plan.result === 'new';
  if (fork) lines.push({ id: 'owner', tone: 'plain', text: t('change.plan.newOwner') });
  if (isNewEnvironment(plan.to.environment)) {
    const host = plan.to.environment.new.place.host;
    lines.push({ id: 'new-environment', tone: 'plain', resources: host === null ? [] : [{ label: host.name, subject: { kind: 'host', host } }], text: t('change.plan.newEnvironment', { name: plan.to.environment.new.name, host: host?.name ?? '' }) });
  }
  // What an environment not made yet will offer is its template's promise: checked before anything is committed.
  if (plan.to.predicted) lines.push({ id: 'predicted', tone: 'plain', text: t('change.plan.predicted') });
  if (plan.to.provider.id !== plan.from.provider.id) lines.push({ id: 'provider', tone: 'plain', resources: [{ label: planProviderName(plan, 'from', context, t), subject: { kind: 'provider', provider: plan.from.provider } }, { label: planProviderName(plan, 'to', context, t), subject: { kind: 'provider', provider: plan.to.provider } }], text: t('change.plan.provider', { to: planProviderName(plan, 'to', context, t), from: planProviderName(plan, 'from', context, t) }) });
  if (plan.to.agent.id !== plan.from.agent.id) lines.push({ id: 'agent', tone: 'plain', resources: [{ label: plan.from.agent.name, subject: { kind: 'agent', agent: plan.from.agent } }, { label: plan.to.agent.name, subject: { kind: 'agent', agent: plan.to.agent } }], text: t('change.plan.agent', { to: plan.to.agent.name, from: plan.from.agent.name }) });
  if (plan.to.model !== context.thread.model || plan.to.effort !== context.thread.effort) {
    const before = modelWords(modelLabel(context.thread.model, plan.from.provider.id === plan.to.provider.id ? context.models ?? [] : [], context.thread.modelDisplay, t), effortLabel(context.thread.effort, t));
    const after = targetModelWords(plan, context, t);
    const resources: NamedResource[] = [{ label: after, subject: { kind: 'model', providerId: plan.to.provider.id, model: { model_name: plan.to.model, display_name: targetModelName(plan, context, t), parameters: context.models?.find((model) => model.model_name === plan.to.model)?.parameters ?? {} }, effort: plan.to.effort } }];
    if (context.thread.model !== null) resources.unshift({ label: before, subject: { kind: 'model', providerId: plan.from.provider.id, model: { model_name: context.thread.model, display_name: context.thread.modelDisplay ?? '', parameters: plan.from.provider.id === plan.to.provider.id ? context.models?.find((model) => model.model_name === context.thread.model)?.parameters ?? {} : {} }, effort: context.thread.effort } });
    lines.push({ id: 'model', resources, tone: 'plain', text: t('changeCompact.modelChange', { from: before, to: after }) });
  }
  if (!isNewEnvironment(plan.to.environment) && plan.from.environment !== null && plan.to.environment.id !== plan.from.environment.id) {
    lines.push({ id: 'environment', tone: 'plain', resources: [{ label: plan.from.environment.name, subject: { kind: 'environment', environment: plan.from.environment } }, { label: plan.to.environment.name, subject: { kind: 'environment', environment: plan.to.environment } }], text: t('change.plan.environment', { to: plan.to.environment.name, from: environmentName(plan.from.environment) }) });
  }
  lines.push({ id: 'history', tone: 'plain', text: historyLine(plan, t), hint: 'history' });
  if (plan.files !== null) lines.push(...filesLines(plan.files, context.locale, t));
  if (plan.memory !== null) {
    lines.push({
      id: 'memory',
      tone: 'plain',
      text: t(plan.memory.carried ? 'change.plan.memoryAlong' : 'change.plan.memoryLeft', { count: plan.memory.files, size: formatBytes(plan.memory.bytes, context.locale) }),
    });
  }
  lines.push(...mountLines(plan.mounts.filter((one) => one.action !== 'detach'), t));
  lines.push(...copyLines(plan.mounts, context.locale, t));
  lines.push(...mcpLines(plan.mcp.filter((server) => server.fate === 'new'), t));
  return lines;
}

/** How the history travels, as one line: the way, and how well it is kept. */
export function historyLine(plan: ForkPlan, t: TFunction): string {
  const how = fidelityKey(plan.history.method, plan.history.report.fidelity);
  const mode = plan.history.mode;
  const base = t(`change.history.badge.${how}`);
  if (mode === 'none') return t('change.plan.historyNone');
  if (mode === 'summary') return t('change.plan.historySummary');
  if (mode === 'range') return t('change.plan.historyRange', { badge: base, turns: plan.history.turns });
  return t('change.plan.history', { badge: base });
}

/** Whom the result belongs to and what stays: for a switch, whose the thread stays; what the person left alone; its page. */
export function staysOf(plan: ForkPlan, context: PlanContext, t: TFunction): Line[] {
  const lines: Line[] = [];
  const fork = plan.result === 'new';
  if (fork) {
    lines.push({ id: 'source', tone: 'plain', text: t('change.plan.sourceUntouched') });
  } else {
    const owner = plan.owner.after;
    lines.push({
      id: 'owner',
      tone: 'plain',
      text: owner.id === context.me ? t('change.plan.staysYours') : t('change.plan.staysOwners', { name: owner.display_name }),
    });
    if (plan.to.provider.id === plan.from.provider.id) lines.push({ id: 'same-provider', tone: 'plain', resources: [{ label: planProviderName(plan, 'from', context, t), subject: { kind: 'provider', provider: plan.from.provider } }], text: t('change.plan.staysOn', { provider: planProviderName(plan, 'from', context, t) }) });
    if (!isNewEnvironment(plan.to.environment) && plan.from.environment !== null && plan.to.environment.id === plan.from.environment.id) {
      lines.push({ id: 'same-environment', tone: 'plain', resources: [{ label: plan.from.environment.name, subject: { kind: 'environment', environment: plan.from.environment } }], text: t('change.plan.staysIn', { environment: plan.from.environment.name }) });
    }
  }
  if (plan.access.differs.length === 0) {
    lines.push({ id: 'page', tone: 'plain', text: fork ? t('change.plan.pageCarried') : t('change.plan.pageStays'), hint: 'page' });
  } else {
    for (const difference of plan.access.differs) {
      lines.push({
        id: `differs-${difference.permission}`,
        tone: 'warning',
        text: t('change.plan.pageDiffers', { permission: permissionLabel(difference.permission), before: holderNames(difference.before, t), after: holderNames(difference.after, t) }),
        hint: 'page',
      });
    }
  }
  return lines;
}

/** What stops and what is taken away: the old side in one exact sentence, processes, mounts the new place does not have. */
export function endsOf(plan: ForkPlan, context: PlanContext, t: TFunction): Line[] {
  const lines: Line[] = [];
  const left = plan.leaves;
  if (left !== null && left.environment !== null) {
    const { action, environment, processes } = left.environment;
    const resources: NamedResource[] = [{ label: environment.name, subject: { kind: 'environment', environment } }];
    lines.push({ id: 'old-side', resources, tone: action === 'remove' ? 'warning' : 'plain', text: t(`change.plan.old.${action}`, { name: environment.name }), hint: 'leaves' });
    if (processes > 0 && plan.options.stop_processes) lines.push({ id: 'processes', resources, tone: 'plain', text: t('change.plan.processes', { count: processes, name: environment.name }) });
  }
  if (left?.session === true) lines.push({ id: 'session', tone: 'plain', text: t('change.plan.oldSession') });
  lines.push(...mountLines(plan.mounts.filter((one) => one.action === 'detach'), t));
  lines.push(...mcpLines(plan.mcp.filter((server) => server.fate === 'lost' || server.fate === 'unavailable'), t));
  if (lines.length === 0) lines.push({ id: 'nothing', tone: 'plain', text: context.kind === 'fork' ? t('change.plan.endsNothingFork') : t('change.plan.endsNothing') });
  return lines;
}

// ---------------------------------------------------------------- blockers

/** A blocker in words of the page, with what would lift it; the server's own sentence stands where this part has no words for the code. */
export function blockerWords(blocker: PlanBlocker, plan: { to?: ForkPlan['to'] } | null, t: TFunction): { text: string; lifts: string | null } {
  const provider = plan?.to?.provider.name ?? '';
  const model = plan?.to?.model ?? '';
  const effort = plan?.to?.effort ?? '';
  if (blocker.code.startsWith('runtime_limit.')) {
    const key = blocker.code.slice('runtime_limit.'.length);
    const why = t(`isolation.limits.${key}.why`, { defaultValue: blocker.message });
    return { text: why === '' ? why : why.charAt(0).toUpperCase() + why.slice(1), lifts: null };
  }
  switch (blocker.code) {
    case 'approval_mode_unsupported':
    case 'approval_mode_not_allowed':
    case 'approval_mode_locked':
      return { text: t(`change.failure.${blocker.code}`), lifts: null };
    case 'nothing_to_change':
      return { text: t('change.blocker.nothing'), lifts: t('change.blocker.nothingLifts') };
    case 'already_running':
      return { text: t('change.blocker.running'), lifts: t('change.blocker.runningLifts') };
    case 'provider_not_ready':
      return { text: t('change.blocker.providerNotReady', { provider }), lifts: t('change.blocker.providerNotReadyLifts') };
    case 'provider_disabled':
      return { text: t('change.blocker.providerDisabled'), lifts: t('change.blocker.providerNotReadyLifts') };
    case 'environment_disabled':
      return { text: t('change.blocker.environmentDisabled'), lifts: t('change.blocker.placeLifts') };
    case 'model_not_allowed':
      return { text: t('change.blocker.modelNotAllowed', { model, provider }), lifts: t('change.blocker.modelLifts') };
    case 'effort_not_listed':
      return { text: t('change.blocker.effortNotListed', { effort, model }), lifts: t('change.blocker.effortLifts') };
    case 'no_host':
      return { text: t('change.blocker.noHost'), lifts: t('change.blocker.placeLifts') };
    case 'no_capacity':
      return { text: t('change.blocker.noCapacity', { detail: blocker.message }), lifts: t('change.blocker.placeLifts') };
    case 'removal_scheduled':
      return { text: t('change.blocker.removal'), lifts: t('change.blocker.removalLifts') };
    case 'unbound':
      return { text: t('change.blocker.unbound'), lifts: null };
    case 'access.denied':
    case 'access':
      return { text: t('change.blocker.access'), lifts: blocker.missing === undefined || blocker.missing === null || blocker.missing.length === 0 ? null : t('change.blocker.accessLifts') };
    default:
      return { text: t(`change.blocker.${blocker.code}`, { defaultValue: blocker.message }), lifts: null };
  }
}

/** The people a blocker says to ask, for the line that offers it. */
export function askOf(blocker: PlanBlocker): UserRef[] {
  return (blocker.missing ?? []).flatMap((missing) => missing.ask);
}

/** The place a person has in a refused plan, named: for a line such as "Alice, Bo and 2 others". */
export function nodeName(ref: NodeRef | null): string {
  return ref?.name ?? '';
}
