/** The build refuses broken help rather than shipping a link or tutorial to an absent control. */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';

import { checkHelpContent, type HelpContentInput } from '../src/help/contentCheck';
import { makeGuide } from '../src/help/guide';
import { TARGETS } from '../src/help/targets';
import { TUTORIALS } from '../src/help/tutorials';
import { SETTINGS_REGISTRY } from '../src/generated/settings-registry';

const sourceRoot = fileURLToPath(new URL('../src/', import.meta.url));
const guideRoot = fileURLToPath(new URL('../../docs/user/', import.meta.url));
const providerRoot = fileURLToPath(new URL('../../docs/development/extensions/providers/', import.meta.url));
const adapterGuide = fileURLToPath(new URL('../../docs/development/extensions/adapters.md', import.meta.url));
const agentContract = fileURLToPath(new URL('../../crates/contracts/agent/README.md', import.meta.url));
const programContract = fileURLToPath(new URL('../../crates/contracts/agent-programs/README.md', import.meta.url));

function sourceFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    if (entry.name === 'generated') return [];
    const path = join(directory, entry.name);
    return entry.isDirectory() ? sourceFiles(path) : /\.tsx?$/.test(entry.name) && entry.name !== 'routeTree.gen.ts' ? [path] : [];
  });
}

export function readHelpContent(): HelpContentInput {
  const sources = Object.fromEntries(sourceFiles(sourceRoot).map((path) => [relative(sourceRoot, path).replaceAll('\\', '/'), readFileSync(path, 'utf8')]));
  const articles = Object.fromEntries(readdirSync(guideRoot).filter((name) => name.endsWith('.md')).map((name) => [name, readFileSync(join(guideRoot, name), 'utf8')]));
  const chineseRoot = join(guideRoot, 'zh-CN');
  const chinese = Object.fromEntries(readdirSync(chineseRoot).filter((name) => name.endsWith('.md')).map((name) => [name, readFileSync(join(chineseRoot, name), 'utf8')]));
  const providers = Object.fromEntries(readdirSync(providerRoot).filter((name) => name.endsWith('.md')).map((name) => [`providers/${name}`, readFileSync(join(providerRoot, name), 'utf8')]));
  const schemaRoot = join(providerRoot, 'schema');
  const files = { ...Object.fromEntries((existsSync(schemaRoot) ? readdirSync(schemaRoot) : []).filter((name) => name.endsWith('.json')).map((name) => [`schema/${name}`, join(schemaRoot, name)])), ...(existsSync(agentContract) ? { '../../../crates/contracts/agent/README.md': agentContract } : {}), ...(existsSync(programContract) ? { '../../../crates/contracts/agent-programs/README.md': programContract } : {}) };
  const adapters = { 'adapters.md': readFileSync(adapterGuide, 'utf8') };
  const routes = Object.entries(sources).filter(([path]) => path.startsWith('routes/')).flatMap(([, source]) => [...source.matchAll(/createFileRoute\(\s*['"]([^'"]+)['"]\s*\)/g)].flatMap((match) => match[1] === undefined ? [] : [match[1]]));
  const catalogues: unknown[] = ['en', 'zh-CN'].map((language) => JSON.parse(readFileSync(join(sourceRoot, 'i18n', `${language}.json`), 'utf8')) as unknown);
  return { guide: makeGuide({ ...articles, ...providers, ...adapters }, files, { 'zh-CN': chinese }), userArticles: makeGuide(articles).articles.map((article) => article.id), tutorials: TUTORIALS, targets: TARGETS, sources, catalogues, routes, settings: SETTINGS_REGISTRY };
}

export function helpContentPlugin(): Plugin {
  return { name: 'orch-help-content', apply: 'build', buildStart() {
    const failures = checkHelpContent(readHelpContent());
    if (failures.length > 0) this.error(`Invalid shipped help:\n${failures.join('\n')}`);
  } };
}
