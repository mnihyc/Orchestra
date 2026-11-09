/** Publish only independently authorized Defaults members into their existing readers. */
import type { Query, QueryClient, QueryKey } from '@tanstack/react-query';
import { applyRow, record } from '@/api/cache';
import { ApiError, client } from '@/api/client';
import { peopleHintCut } from '@/api/peopleChanges';
import { nodeKey } from '@/api/nodes';
import { providerKey } from '@/api/resources';
import { registryQuery, secretsKey, settingsKey } from '@/api/settings';
import { availableShape, lockAbove } from '@/features/settings/settingsModel';
import { editableValue } from '@/features/settings/settingParts';
import { settingShape } from '@/features/settings/settingText';
import type { Node, NodeChatOpeningGetQuery, NodeDefaultsOpening, Problem, ResolvedSetting, SettingDef } from '@/generated/api-types';

export type DefaultsDemand = Pick<NodeChatOpeningGetQuery, 'defaults' | 'settings_only' | 'secret_names' | 'default_provider_name' | 'embeddings_provider_name'>;
export const providerDefaultKey = (id: string, reader = defaultProviderReader()) => [...providerKey(id), 'default-choice', reader] as const;
export function defaultProviderReader(): string { const state = client.session.getState(); return `${state.userId ?? ''}:${state.readerVersion ?? 0}`; }

function errorFor(problem: Problem, seconds: number | null): ApiError {
  return new ApiError(problem, seconds !== null && Number.isFinite(seconds) && seconds >= 0 ? new Date(Date.now() + seconds * 1_000) : null);
}
function publish(cache: QueryClient, key: QueryKey, data: unknown, problem: Problem | null, retry: number | null): void {
  if (data !== null && data !== undefined && problem === null) {
    const query = cache.getQueryCache().find({ queryKey: key, exact: true });
    if (query?.state.fetchStatus === 'fetching' && !isJoinedConsumer(query)) void query.cancel({ revert: false, silent: true });
    cache.setQueryData(key, data);
  } else if (record(problem) && typeof problem.code === 'string' && typeof problem.status === 'number') {
    const query = cache.getQueryCache().build(cache, cache.defaultQueryOptions({ queryKey: key }));
    if (!isJoinedConsumer(query)) void query.cancel({ revert: false, silent: true });
    query.setState({ data: undefined, dataUpdatedAt: 0, error: errorFor(problem, retry), status: 'error',
      fetchStatus: isJoinedConsumer(query) ? query.state.fetchStatus : 'idle' });
  }
}
function selected(setting: ResolvedSetting, key: string): unknown {
  return key === 'search.semantic.embeddings' && record(setting.value) ? setting.value.provider_id : setting.value;
}

/** A missing or foreign envelope falls back to the same native queries; it is never an empty success. */
export function seedNodeDefaults(cache: QueryClient, id: string, value: NodeDefaultsOpening | null | undefined, demand: DefaultsDemand, reader: string): boolean {
  if (demand.defaults !== true || !record(value) || !record(value.node) || value.node.id !== id
    || !['org', 'project', 'topic'].includes(value.node.kind) || !record(value.node.access) || !Array.isArray(value.node.access.permissions)
    || !Number.isSafeInteger(value.node.revision) || value.node.revision < 0) return false;
  const previous = cache.getQueryData<Node>(nodeKey(id));
  if (previous !== undefined && previous.revision > value.node.revision) return false;
  if (Array.isArray(value.registry) || value.registry_problem !== null) publish(cache, registryQuery.queryKey, value.registry, value.registry_problem, value.registry_retry_after);
  if (Array.isArray(value.settings) || value.settings_problem !== null) publish(cache, settingsKey(id), value.settings, value.settings_problem, value.settings_retry_after);
  if (demand.secret_names === true && (Array.isArray(value.secrets) || value.secrets_problem !== null)) {
    publish(cache, secretsKey({ kind: 'node', id }), value.secrets, value.secrets_problem, value.secrets_retry_after);
  }
  if (Array.isArray(value.providers) && Array.isArray(value.settings)) {
    for (const entry of value.providers) {
      if (!record(entry) || typeof entry.id !== 'string' || typeof entry.setting !== 'string') continue;
      const setting = value.settings.find((row) => row.key === entry.setting);
      const definition = value.registry?.find((def) => def.key === entry.setting);
      const wanted = entry.setting === 'agent.default_provider_id' && demand.default_provider_name === true
        || entry.setting === 'search.semantic.embeddings' && value.node.kind === 'org' && demand.embeddings_provider_name === true;
      if (!wanted || setting === undefined || definition === undefined || selected(setting, entry.setting) !== entry.id) continue;
      if (entry.provider !== null && (!record(entry.provider) || entry.provider.id !== entry.id)) continue;
      const choice = entry.setting === 'agent.default_provider_id' && setting.writable && lockAbove(setting, { kind: 'node', id }) === null;
      const key = choice ? providerDefaultKey(entry.id, reader) : providerKey(entry.id);
      // An explicit native denial retires another current reader of the same rich name too.
      if (entry.problem !== null) {
        for (const other of [providerKey(entry.id), providerDefaultKey(entry.id, reader)]) {
          if (cache.getQueryState(other) !== undefined) publish(cache, other, null, entry.problem, entry.retry_after);
        }
      }
      const shown = editableValue(definition, setting, availableShape(settingShape(definition), setting.available_values));
      if (!choice || shown === entry.id) publish(cache, key, entry.provider, entry.problem, entry.retry_after);
    }
  }
  // All metadata is installed before the Node can mount its editor or the Chat can publish.
  publish(cache, nodeKey(id), value.node, null, null);
  applyRow(cache, { kind: 'node', id, value: value.node });
  return true;
}

// Request receipts live only on the existing source/consumer Query objects, never in a second store.
const OPENING_READ = Symbol('Defaults opening read');
const JOINED_READ = Symbol('Joined Defaults read');
class OpeningRead {
  readonly cache: QueryClient;
  readonly query: Query;
  readonly nodeId: string;
  readonly signal: AbortSignal;
  promise: Promise<unknown> | undefined;
  settled = false;
  consumers = 0;
  readonly userId: string | null;
  readonly reader: number | undefined;
  readonly owns: () => boolean;
  readonly cut: number;
  readonly wantsDefaults: boolean;
  projection: NodeDefaultsOpening | undefined;
  constructor(cache: QueryClient, query: Query, nodeId: string, signal: AbortSignal, promise: Promise<unknown>, owns: () => boolean, wantsDefaults: boolean) {
    this.cache = cache; this.query = query; this.nodeId = nodeId; this.signal = signal; this.promise = promise; this.owns = owns; this.cut = peopleHintCut(cache); this.wantsDefaults = wantsDefaults;
    const current = client.session.getState(); this.userId = current.userId; this.reader = current.readerVersion;
    const done = () => { this.settled = true; this.release(); }; void promise.then(done, done);
  }
  current(): boolean {
    const now = client.session.getState();
    return !this.signal.aborted && now.userId === this.userId && now.readerVersion === this.reader && this.owns()
      && this.cache.getQueryCache().find({ queryKey: this.query.queryKey, exact: true }) === this.query
      && receipt(this.query, OPENING_READ) === this && (this.query.promise === undefined ? this.settled : this.query.promise === this.promise);
  }
  release(): void { if (this.settled && this.consumers === 0) { this.promise = undefined; this.projection = undefined; } }
}
function receipt(query: Query, name: symbol): OpeningRead | undefined {
  const value: unknown = Reflect.get(query, name);
  return value instanceof OpeningRead ? value : undefined;
}
function tag(query: Query, name: symbol, read: OpeningRead): void {
  Object.defineProperty(query, name, { value: read, writable: true, configurable: true });
}
function isJoinedConsumer(query: Query): boolean { return receipt(query, JOINED_READ)?.current() === true; }
export function beginDefaultsOpening(cache: QueryClient, key: QueryKey, nodeId: string, signal: AbortSignal, owns: () => boolean, wantsDefaults: boolean): OpeningRead | undefined {
  const query = cache.getQueryCache().find({ queryKey: key, exact: true }), pending = query?.promise;
  if (query === undefined || pending === undefined) return undefined;
  const read = new OpeningRead(cache, query, nodeId, signal, pending, owns, wantsDefaults); tag(query, OPENING_READ, read); return read;
}
/** Failed and ordinary attempts retain their generation independently of native-read cancellation. */
export function hasUnattemptedPeopleHint(cache: QueryClient, key: QueryKey): boolean {
  const query = cache.getQueryCache().find({ queryKey: key, exact: true }), read = query === undefined ? undefined : receipt(query, OPENING_READ);
  const current = client.session.getState();
  return read?.userId === current.userId && read.reader === current.readerVersion && peopleHintCut(cache) > read.cut;
}
export function hasDefaultsOpeningOwner(cache: QueryClient, key: QueryKey): boolean {
  const query = cache.getQueryCache().find({ queryKey: key, exact: true }), read = query === undefined ? undefined : receipt(query, OPENING_READ);
  const current = client.session.getState();
  return read?.userId === current.userId && read.reader === current.readerVersion && read.owns();
}
async function sharedProjection(cache: QueryClient, key: QueryKey, signal: AbortSignal, nodeId?: string): Promise<{ read: OpeningRead; value: NodeDefaultsOpening } | undefined> {
  const before = client.session.getState();
  const query = cache.getQueryCache().getAll().find((query) => {
    if (query.queryKey[0] !== 'node-chat-opening' || query.state.fetchStatus !== 'fetching') return false;
    const read = receipt(query, OPENING_READ);
    if (read?.promise === undefined) return false;
    return read.wantsDefaults && read.nodeId === query.queryKey[1] && (nodeId === undefined || read.nodeId === nodeId) && read.current();
  });
  const read = query === undefined ? undefined : receipt(query, OPENING_READ);
  const pending = read?.promise;
  if (read === undefined || pending === undefined) return undefined;
  read.consumers += 1;
  const consumer = cache.getQueryCache().find({ queryKey: key, exact: true });
  if (consumer !== undefined) tag(consumer, JOINED_READ, read);
  try {
    signal.throwIfAborted();
    await new Promise<void>((resolve, reject) => {
      const abort = () => reject(new DOMException('The consumer was closed.', 'AbortError'));
      signal.addEventListener('abort', abort, { once: true });
      void pending.then(() => resolve(), () => resolve()).finally(() => signal.removeEventListener('abort', abort));
    });
    signal.throwIfAborted();
    const now = client.session.getState();
    if (now.userId !== before.userId || now.readerVersion !== before.readerVersion) throw new DOMException('The requesting reader changed.', 'AbortError');
    if (!read.current() || read.projection === undefined) return undefined;
    return { read, value: read.projection };
  } finally {
    if (consumer !== undefined && receipt(consumer, JOINED_READ) === read) Reflect.deleteProperty(consumer, JOINED_READ);
    read.consumers -= 1; read.release();
  }
}
/** A globally shared registry still belongs to this exact current, explicitly owned HTTP attempt. */
export async function joinedDefaultsRegistry(cache: QueryClient, key: QueryKey, signal: AbortSignal): Promise<SettingDef[] | undefined> {
  const projection = await sharedProjection(cache, key, signal);
  if (projection === undefined) return undefined;
  if (Array.isArray(projection.value.registry)) return projection.value.registry;
  const problem = projection.value.registry_problem;
  if (record(problem) && typeof problem.code === 'string' && typeof problem.status === 'number') throw errorFor(problem, projection.value.registry_retry_after);
  return undefined;
}
/** Fresh authority repair uses only a Node independently admitted in its current hint generation. */
export async function joinedDefaultsNode(cache: QueryClient, id: string, signal: AbortSignal): Promise<Node | undefined> {
  const projection = await sharedProjection(cache, nodeKey(id), signal, id);
  if (projection === undefined) return undefined;
  return peopleHintCut(cache) !== projection.read.cut ? undefined : projection.value.node;
}
