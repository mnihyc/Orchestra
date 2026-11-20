/** Stable device-choice categories name observations without retaining a private path or value. */
export const VIEW_CHOICE_KINDS = ['global', 'org', 'project', 'topic', 'thread', 'document', 'page', 'resource-providers', 'resource-agents', 'resource-environments', 'resource-hosts', 'resource-machines', 'resource-templates', 'provider-endpoints', 'chat', 'help', 'search', 'approvals', 'ports', 'storage', 'trace', 'navigation', 'onboarding', 'links', 'import-review', 'import-report', 'usage', 'file-viewers', 'everything', 'view-window'] as const;
export const VIEW_CHOICE_FIELDS = ['tab', 'view', 'rightOpen', 'rightSize', 'mobileView', 'mobileTab', 'listOrigin', 'scope', 'advanced', 'endpointAdvanced', 'part', 'setup', 'filters'] as const;
export type DeviceChoiceKind = (typeof VIEW_CHOICE_KINDS)[number] | 'other';
export type DeviceChoiceOrigin = 'address' | 'item' | 'kind' | 'default' | 'page';
export type DeviceChoiceRead = 'available' | 'unavailable' | 'invalid' | 'memory';
export type DeviceChoiceSave = 'saved' | 'failed' | 'not_checked';

export interface DeviceChoiceObservation<Value = unknown> {
  reader: string;
  scope: { kind: string; key: string; thing?: string };
  chosen: unknown;
  effective: Value;
  saved: unknown;
  origin: DeviceChoiceOrigin;
  saved_choice_availability: 'available' | 'unavailable' | 'unreported';
  read_result: DeviceChoiceRead;
  write_result: DeviceChoiceSave;
}
export interface DeviceChoiceFailure { kind: DeviceChoiceKind; write_result: 'failed' }

export function deviceChoiceKind(kind: string): DeviceChoiceKind {
  const root = kind.split('.')[0];
  if (root === 'providerUsage') return 'usage';
  return VIEW_CHOICE_KINDS.find((known) => known === root) ?? 'other';
}

// A control receives the receipt of its exact writer, without another storage read or a global list of values.
const choices = new WeakMap<object, DeviceChoiceObservation>();
const fields = new WeakMap<object, ReadonlyMap<string, DeviceChoiceObservation>>();
export function bindDeviceChoice<Writer extends object>(writer: Writer, observation: DeviceChoiceObservation | undefined): Writer {
  if (observation === undefined) choices.delete(writer); else choices.set(writer, observation);
  return writer;
}
export function deviceChoiceOf(writer: object): DeviceChoiceObservation | undefined { return choices.get(writer); }
/** Membership is judged by the actual control; a valid local override stays separate from a missing saved option. */
export function offeredDeviceChoice(receipt: DeviceChoiceObservation | undefined, effective: unknown, offered: readonly unknown[]): DeviceChoiceObservation | undefined {
  if (receipt === undefined) return undefined;
  const fallback = !Object.is(receipt.effective, effective) && !offered.includes(receipt.effective);
  return { ...receipt, effective, origin: fallback ? 'default' : Object.is(receipt.effective, effective) ? receipt.origin : 'page',
    saved_choice_availability: receipt.saved === undefined ? receipt.saved_choice_availability : offered.includes(receipt.saved) ? 'available' : 'unavailable' };
}
/** A page that wraps a writer for navigation keeps the same operation's receipt. */
export function carryDeviceChoice<Writer extends object>(writer: Writer, source: object, field?: string): Writer {
  if (field === undefined) {
    const receipts = fields.get(source);
    if (receipts === undefined) fields.delete(writer); else fields.set(writer, receipts);
  }
  return bindDeviceChoice(writer, field === undefined ? deviceChoiceOf(source) : fields.get(source)?.get(field));
}
/** A control for one part of a compound choice uses that part's observed value, without another read. */
export function mapDeviceChoice<Writer extends object>(writer: Writer, source: object, map: (value: unknown) => unknown): Writer {
  const receipt = deviceChoiceOf(source);
  return bindDeviceChoice(writer, receipt === undefined ? undefined : { ...receipt, chosen: map(receipt.chosen), effective: map(receipt.effective), saved: receipt.saved === undefined ? undefined : map(receipt.saved) });
}
/** A compound page writer passes each existing field's own receipt to that field's control. */
export function bindDeviceChoiceFields<Writer extends object>(writer: Writer, sources: Readonly<Record<string, object>>): Writer {
  fields.set(writer, new Map(Object.entries(sources).flatMap(([key, source]) => {
    const fact = deviceChoiceOf(source);
    return fact === undefined ? [] : [[key, fact] as const];
  })));
  return writer;
}
