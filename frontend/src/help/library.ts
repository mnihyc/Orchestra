/** The user guide as the build holds it: every article of `docs/user/`, read into data.
 *
 * Loaded on first use (`index.ts`), as a part of the build of its own.
 */
import { makeGuide } from '@/help/guide';

const sources = import.meta.glob<string>('../../../docs/user/*.md', { query: '?raw', import: 'default', eager: true });
const chinese = import.meta.glob<string>('../../../docs/user/zh-CN/*.md', { query: '?raw', import: 'default', eager: true });
const providers = import.meta.glob<string>('../../../docs/development/extensions/providers/*.md', { query: '?raw', import: 'default', eager: true });
const adapters = import.meta.glob<string>('../../../docs/development/extensions/adapters.md', { query: '?raw', import: 'default', eager: true });
const schemas = import.meta.glob<string>('../../../docs/development/extensions/providers/schema/*.json', { query: '?url&no-inline', import: 'default', eager: true });
const contracts = import.meta.glob<string>('../../../crates/contracts/agent/README.md', { query: '?url&no-inline', import: 'default', eager: true });
const programContracts = import.meta.glob<string>('../../../crates/contracts/agent-programs/README.md', { query: '?url&no-inline', import: 'default', eager: true });
const files = Object.fromEntries([
  ...Object.entries(schemas).map(([path, url]): [string, string] => [`schema/${path.split('/').at(-1) ?? ''}`, url]),
  ...Object.entries(contracts).map(([, url]): [string, string] => ['../../../crates/contracts/agent/README.md', url]),
  ...Object.entries(programContracts).map(([, url]): [string, string] => ['../../../crates/contracts/agent-programs/README.md', url]),
]);

export const guide = makeGuide({ ...sources, ...providers, ...adapters }, files, { 'zh-CN': chinese });
