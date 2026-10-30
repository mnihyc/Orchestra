/** The fake API's shipped executor declarations, read from the same manifest data as the release. */
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { ExecutorInfo, MachineExecutor } from '../../src/generated/api-types.ts';

type Manifest = Pick<ExecutorInfo, 'kind' | 'title'> & Partial<Pick<ExecutorInfo, 'program' | 'tool_sets'>>;
const here = dirname(fileURLToPath(import.meta.url));
const manifests = ['codex-exec-server', 'orch-exec'].map((name) => JSON.parse(readFileSync(resolve(here, '../../../crates/contracts/agent-programs/executors', `${name}.json`), 'utf8')) as Manifest);

export const EXECUTORS: ExecutorInfo[] = manifests.map((manifest) => {
  const toolSets = manifest.tool_sets ?? [];
  return { kind: manifest.kind, title: manifest.title, program: manifest.program ?? null, tool_sets: toolSets,
    offers: toolSets.length === 0
      ? [{ value: manifest.kind, alias: null, title: manifest.title, summary: manifest.title }]
      : toolSets.map((toolSet) => ({ value: `${manifest.kind}:${toolSet.name}`, alias: toolSet.name, title: toolSet.title, summary: toolSet.summary })),
  };
});

export const MACHINE_EXECUTORS: MachineExecutor[] = EXECUTORS.map((executor) => ({ executor, origin: 'builtin', state: null }));
