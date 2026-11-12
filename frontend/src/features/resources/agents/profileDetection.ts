/** Profile proposals describe reported facts; thread arrangements still use the shared selector. */
import type { HarnessKindInfo, MachineProgram } from '@/generated/api-types';
import { readMachineFacts } from '@/features/resources/machines/facts';

const object = (value: unknown): Record<string, unknown> | null =>
  typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null;
const list = (value: unknown): readonly unknown[] => Array.isArray(value) ? value : [];
const version = (value: unknown): string | null => typeof value === 'string' && value.trim() !== '' && value.trim().toLowerCase() !== 'unknown' ? value : null;
const strings = (value: unknown): string[] | null => Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : null;

/** A ready machine declaration outranks the catalogue. A program's own version is not its harness version. */
export function profileDetection(facts: unknown, kind: string, catalogue: Pick<HarnessKindInfo, 'descriptor'> | undefined, registered?: MachineProgram) {
  const raw = object(facts);
  const reports = registered === undefined ? [
    ...list(object(raw?.programs)?.entries), ...list(raw?.plugins),
  ] : [registered];
  let local: Record<string, unknown> | null = null;
  for (const report of reports) {
    const row = object(report);
    if (row === null || row.kind !== 'agent' && row.kind !== 'adapter' || row.ready === false || row.state === 'failed') continue;
    const descriptor = object(row.descriptor);
    if (descriptor?.kind === kind) { local = descriptor; break; }
  }
  const declared = local ?? object(catalogue?.descriptor);
  const machine = readMachineFacts(facts);
  const installed = machine.harnesses.find((entry) => entry.kind === kind);
  return {
    version: version(installed?.version) ?? version(object(local?.harness)?.version),
    source: local !== null ? 'machine' as const : declared !== null ? 'catalogue' as const : null,
    arrangements: Array.isArray(declared?.arrangements) ? declared.arrangements.flatMap((entry): string[] => {
      const type = object(entry)?.type;
      return typeof type === 'string' ? [type] : [];
    }) : null,
    machineArrangements: strings(machine.policy?.['provider.arrangements']),
  };
}
