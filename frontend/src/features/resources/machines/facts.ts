/** What a machine reports of itself, read with care: the facts are what its connector wrote, and a machine that has not reported yet has none.
 */
import type { MachineOs } from '@/generated/api-types';
import type { TFunction } from 'i18next';
import { formatNumber } from '@/lib/format';
import { reportedAdapterName } from '@/features/resources/machines/programNames';

/** The systems a machine runs, by their names: a name is the same in every language. */
export const OS_NAMES: Record<MachineOs, string> = { linux: 'Linux', macos: 'macOS', windows: 'Windows' };

/** Native architecture codes remain installer values; ordinary facts use familiar processor names. */
export function architectureName(value: string, t: TFunction): string {
  if (value === 'x86_64' || value === 'amd64' || value === 'x64') return t('choiceCatalog.architecture.x86_64.name');
  if (value === 'aarch64' || value === 'arm64') return t('choiceCatalog.architecture.aarch64.name');
  return t('machines.otherProcessor');
}

/** A round trip above this many milliseconds marks a link as slow: whoever wonders why a file or a shell there is slow reads the reason. */
export const SLOW_LINK_MS = 150;

export interface FoundHarness {
  title?:string;
  kind: string;
  path: string;
  version: string;
  daemon: boolean;
}

export interface MachineAdapter {
  title?:string;
  kind: string;
  builtin: boolean;
  needsBinary?:boolean;
}

export interface MachinePlugin {
  kind: string;
  name: string;
  version: string;
  state: string;
}

export interface MachineLink {
  upBps: number;
  downBps: number;
  roundTripMs: number;
}

export interface MachineFactsView {
  dedicated: boolean;
  hostname: string | null;
  osVersion: string | null;
  cpus: number | null;
  memoryBytes: number | null;
  harnesses: FoundHarness[];
  adapters: MachineAdapter[];
  plugins: MachinePlugin[];
  link: MachineLink | null;
  clockOffsetMs: number | null;
  /** The limits its owner set on it, as reported; `null` for a machine that has none of its own. */
  policy: Record<string, unknown> | null;
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);
const text = (value: unknown): string | null => (typeof value === 'string' ? value : null);
const count = (value: unknown): number | null => (typeof value === 'number' && Number.isFinite(value) ? value : null);

function list<Item>(value: unknown, read: (entry: Record<string, unknown>) => Item | null): Item[] {
  if (!Array.isArray(value)) return [];
  const found: Item[] = [];
  for (const entry of value) {
    const item = isRecord(entry) ? read(entry) : null;
    if (item !== null) found.push(item);
  }
  return found;
}

/** Reads the facts a connector reported; whatever is missing or of the wrong kind is left out. */
export function readMachineFacts(facts: unknown): MachineFactsView {
  const source = isRecord(facts) ? facts : {};
  const rates = isRecord(source.rates) ? source.rates : null;
  const up = rates === null ? null : count(rates.up_bps);
  const down = rates === null ? null : count(rates.down_bps);
  const trip = rates === null ? null : count(rates.round_trip_ms);
  return {
    dedicated: [source.environment_descriptor,source.provider_runtime_descriptor].some((descriptor) => isRecord(descriptor) && descriptor.confinement === 'isolated' && descriptor.confinement_by === 'machine'),
    hostname: text(source.hostname),
    osVersion: text(source.os_version),
    cpus: count(source.cpus),
    memoryBytes: count(source.memory_bytes),
    harnesses: list(source.harnesses, (entry) => {
      const kind = text(entry.kind);
      return kind === null ? null : { kind, path: text(entry.path) ?? '', version: text(entry.version) ?? '', daemon: entry.daemon === true };
    }),
    adapters: list(source.adapters, (entry) => {
      const kind = text(entry.kind);
      return kind === null ? null : { kind, builtin: entry.builtin === true,needsBinary:entry.needs_binary!==false,title:reportedAdapterName(source,kind) ?? undefined };
    }),
    plugins: list(source.plugins, (entry) => {
      const kind = text(entry.kind);
      const name = text(entry.name);
      return kind === null || name === null ? null : { kind, name, version: text(entry.version) ?? '', state: text(entry.state) ?? '' };
    }),
    link: up === null || down === null || trip === null ? null : { upBps: up, downBps: down, roundTripMs: trip },
    clockOffsetMs: count(source.clock_offset_ms),
    policy: isRecord(source.policy) ? source.policy : null,
  };
}

/** A rate in bits per second written as people read it: "80 bit/s", "12 kbit/s", "0.7 Mbit/s". */
export function formatRate(bitsPerSecond: number, locale: string): string {
  const units = ['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s'] as const;
  let value = Math.max(0, bitsPerSecond);
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const digits = unit === 0 || value >= 10 ? 0 : 1;
  return `${formatNumber(value, locale, { maximumFractionDigits: digits })} ${units[unit] ?? 'bit/s'}`;
}

/** The harnesses a machine can run profiles for: those found there whose kind its connector has an adapter for. */
export function registrableHarnesses(facts: MachineFactsView): FoundHarness[] {
  const kinds = new Set(facts.adapters.map((adapter) => adapter.kind));
  const found=facts.harnesses.filter((harness) => kinds.has(harness.kind)).map((harness)=>({...harness,title:facts.adapters.find((adapter)=>adapter.kind===harness.kind)?.title}));
  return [...found,...facts.adapters.filter((adapter)=>adapter.needsBinary===false && !found.some((harness)=>harness.kind===adapter.kind))
    .map((adapter)=>({kind:adapter.kind,path:'',version:'',daemon:false,title:adapter.title}))];
}

/** The directories the machine allows environments in, as its local policy says. */
export function allowedRoots(facts: MachineFactsView): string[] {
  const roots = facts.policy?.['environment.roots'];
  return Array.isArray(roots) ? roots.filter((root): root is string => typeof root === 'string') : [];
}
