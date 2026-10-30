/** Immutable read tickets: history selection is independent of native execution. */
import type { ThreadContinuationProjection, ThreadPromptSummary, ThreadReadSelector } from '@/generated/api-types';

/** A local window owner is an object, never a native thread ID. */
export type ThreadReadKey = Readonly<{ kind: 'thread'; owner: string } | { kind: 'result'; owner: string }>;
export interface ThreadReadRef {
  readonly threadId: string;
  readonly key: ThreadReadKey;
  /** Full content panes hydrate shared composer controls; previews only hydrate history. */
  readonly controls?: true;
}
export type ThreadReadTarget = string | ThreadReadRef;

export function defaultThreadRead(threadId: string): ThreadReadRef {
  const key: ThreadReadKey = { kind: 'thread',owner: threadId };
  return Object.freeze({ threadId,key: Object.freeze(key) });
}
export function resultThreadRead(threadId: string, tabId: string): ThreadReadRef {
  const key: ThreadReadKey = { kind: 'result',owner: tabId };
  return Object.freeze({ threadId,key: Object.freeze(key) });
}
export function contentThreadRead(threadId: string, tabId: string): ThreadReadRef {
  return Object.freeze({ ...resultThreadRead(threadId, tabId), controls: true });
}
export function threadReadTarget(target: ThreadReadTarget): ThreadReadRef {
  const ref = typeof target === 'string' ? defaultThreadRead(target) : target;
  if (ref.key.kind === 'thread' && ref.key.owner !== ref.threadId) throw threadReadChanged();
  return ref;
}
export function sameThreadRead(one: ThreadReadTarget, other: ThreadReadTarget): boolean {
  const left = threadReadTarget(one), right = threadReadTarget(other);
  return left.threadId === right.threadId && left.key.kind === right.key.kind && left.key.owner === right.key.owner;
}
export function isDefaultThreadRead(target: ThreadReadTarget): boolean { return threadReadTarget(target).key.kind === 'thread'; }

/** Default keys keep legacy route-map access; independent windows occupy a disjoint local namespace. */
export function threadReadCacheKey(target: ThreadReadTarget): string {
  const ref = threadReadTarget(target);
  return ref.key.kind === 'thread' ? ref.threadId : `thread-window:${JSON.stringify([ref.threadId,ref.key.kind,ref.key.owner])}`;
}
export function threadReadRegistryKey(target: ThreadReadTarget): string {
  const ref = threadReadTarget(target);
  return ref.key.kind === 'thread' ? `thread:${ref.threadId}` : threadReadCacheKey(ref);
}

export interface ThreadReadScope {
  readonly ref: ThreadReadRef;
  /** Identity and key belong to the existing thread-cache lifetime, never a retained branch map. */
  readonly identity: object;
  readonly lifetime: number;
  readonly reader: number | undefined;
  readonly controller: AbortController;
  readonly epoch: number;
  readonly selection: string | null;
  readonly projection: ThreadContinuationProjection | null;
  readonly ready: boolean;
  /** A standalone default read has not claimed a whole thread window yet. */
  readonly provisional: boolean;
  readonly opening: boolean;
  /** Once switched, an unscoped legacy response cannot be interpreted as this selection. */
  readonly switched: boolean;
}

export interface ThreadReadCapture {
  readonly ref: ThreadReadRef;
  readonly threadId: string;
  readonly scope: ThreadReadScope;
}

let nextLifetime = 0;

export function newThreadReadScope(reader: number | undefined, ref: ThreadReadRef): ThreadReadScope {
  nextLifetime += 1;
  return { ref, identity: {}, lifetime: nextLifetime, reader, controller: new AbortController(), epoch: 0, selection: null,
    projection: null, ready: false, provisional: true, opening: false, switched: false };
}

/** Only the read identity is immutable; an explicit view may learn newer active-execution metadata. */
export function sameThreadProjection(one: ThreadContinuationProjection, other: ThreadContinuationProjection): boolean {
  return one.viewed_continuation_id === other.viewed_continuation_id
    && one.projection_revision === other.projection_revision && one.follows_active === other.follows_active
    && (!one.follows_active || one.active_continuation_id === other.active_continuation_id
      && one.execution_revision === other.execution_revision);
}

export function matchesThreadCapture(scope: ThreadReadScope | null, captured: ThreadReadCapture): boolean {
  return scope !== null && !scope.controller.signal.aborted && !captured.scope.controller.signal.aborted
    && captured.threadId === scope.ref.threadId && sameThreadRead(scope.ref,captured.ref)
    && scope.identity === captured.scope.identity && scope.epoch === captured.scope.epoch
    && scope.reader === captured.scope.reader && scope.selection === captured.scope.selection;
}

export function matchesThreadProjection(scope: ThreadReadScope, captured: ThreadReadCapture): boolean {
  const before = captured.scope.projection;
  return before === null ? scope.projection === null
    : scope.projection !== null && sameThreadProjection(before, scope.projection);
}

export function threadReadChanged(): DOMException {
  return new DOMException('The thread read selection changed', 'AbortError');
}

/** No explicit id means follow active; adding its current id would silently make it an old view. */
export function threadReadSelector(captured: ThreadReadCapture, positioned = true): ThreadReadSelector {
  return {
    ...(captured.scope.selection === null ? {} : { continuation_id: captured.scope.selection }),
    ...(!positioned || captured.scope.projection === null ? {} : {
      projection_revision: captured.scope.projection.projection_revision,
    }),
  };
}

/** A key cannot be reused after eviction or A -> B -> A, even when the native id happens to repeat. */
export function coldThreadKey(reader: number | undefined, base: readonly unknown[], target?: ThreadReadTarget): readonly unknown[] {
  const owner = target === undefined || isDefaultThreadRead(target) ? [] : ['read-window',threadReadCacheKey(target)];
  return [...base,...owner, 'continuation-read', 'unopened-active', reader ?? 0];
}

export function scopedThreadKey(scope: ThreadReadScope, base: readonly unknown[]): readonly unknown[] {
  if (scope.provisional && !scope.switched && scope.selection === null) return coldThreadKey(scope.reader, base, scope.ref);
  const projection = scope.projection;
  const owner = isDefaultThreadRead(scope.ref) ? [] : ['read-window',threadReadCacheKey(scope.ref)];
  return [...base,...owner, 'continuation-read', scope.lifetime, scope.epoch, scope.selection,
    projection?.viewed_continuation_id ?? null, projection?.projection_revision ?? null,
    projection?.follows_active ?? null, projection?.follows_active === true ? projection.execution_revision : null];
}

export function projectionFitsSelection(scope: ThreadReadScope, projection: ThreadContinuationProjection): boolean {
  return scope.selection === null ? projection.follows_active
    : !projection.follows_active && projection.viewed_continuation_id === scope.selection;
}

export function readThreadProjection(value: unknown): ThreadContinuationProjection | null {
  if (typeof value !== 'object' || value === null) return null;
  if (!('viewed_continuation_id' in value) || typeof value.viewed_continuation_id !== 'string'
    || !('active_continuation_id' in value) || typeof value.active_continuation_id !== 'string'
    || !('projection_revision' in value) || typeof value.projection_revision !== 'number'
    || !Number.isSafeInteger(value.projection_revision)
    || !('execution_revision' in value) || typeof value.execution_revision !== 'number'
    || !Number.isSafeInteger(value.execution_revision)
    || !('follows_active' in value) || typeof value.follows_active !== 'boolean') return null;
  return { viewed_continuation_id: value.viewed_continuation_id, active_continuation_id: value.active_continuation_id,
    projection_revision: value.projection_revision, execution_revision: value.execution_revision,
    follows_active: value.follows_active };
}

/** Weak receipt metadata retains no row/cache of its own and vanishes with the actual returned object.
 * Existing controls put returned rows in new arrays; tagging the row preserves their original ticket. */
const receipts = new WeakMap<object, ThreadReadCapture>();
const promptReceipts = new WeakMap<object, ThreadPromptSummary>();

export function stampPromptReceipt(item: object, summary: ThreadPromptSummary): void { promptReceipts.set(item, summary); }
export function promptReceipt(item: object): ThreadPromptSummary | undefined { return promptReceipts.get(item); }

export function stampThreadReceipt<Value extends object>(value: Value, captured: ThreadReadCapture): Value {
  receipts.set(value, captured);
  return value;
}

export function threadReceipt(value: object): ThreadReadCapture | undefined {
  return receipts.get(value);
}

/** Plan cache keys before publishing the complete thread to mounted controls. */
export function planThreadOpening(scope: ThreadReadScope, projection: ThreadContinuationProjection | null): ThreadReadScope | null {
  if (projection !== null && !projectionFitsSelection(scope, projection)
    || projection === null && (scope.selection !== null || scope.switched || scope.projection !== null)) return null;
  const changed = scope.projection !== null && (projection === null || !sameThreadProjection(scope.projection, projection));
  return { ...scope, projection, ready: true, provisional: false, opening: false,
    epoch: scope.epoch + (changed ? 1 : 0), switched: scope.switched || changed,
    controller: changed ? new AbortController() : scope.controller };
}
