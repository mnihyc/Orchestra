/** What the page's entry holds: every module and package that its static imports reach, as the build leaves them.
 *
 * The build splits each route's screen into a chunk of its own (`autoCodeSplitting`), so a route file keeps only what its
 * address needs (its search check, its static data, its loader) and the imports of those. This walks the same imports: a route
 * file is first put through the router plugin's own split, and what is left of it is followed. Imports that are only types, and
 * dynamic `import()`, are not followed: they are what loads later.
 */
import { existsSync, readFileSync, statSync } from 'node:fs';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import ts from 'typescript';

export interface EntryGraph {
  /** Every module reached, by path under `src` (`ui/CodeView.tsx`), with the module that first imported it. */
  modules: ReadonlyMap<string, string | null>;
  /** Every package reached, by name (`@codemirror/view`), with the module that first imported it. */
  packages: ReadonlyMap<string, string>;
  /** Source bytes of every module reached. */
  bytes: number;
}

const EXTENSIONS = ['.ts', '.tsx', '/index.ts', '/index.tsx', '.json', ''];

type Split = (opts: { code: string; codeSplitGroupings: unknown; targetFramework: string; filename: string; id: string }) => { code: string } | null;

async function routeSplitter(root: string): Promise<(code: string, file: string) => string> {
  const core = join(root, 'node_modules/@tanstack/router-plugin/dist/esm/core');
  const compilers = await import(/* @vite-ignore */ pathToFileURL(join(core, 'code-splitter/compilers.js')).href) as { compileCodeSplitReferenceRoute: Split };
  const constants = await import(/* @vite-ignore */ pathToFileURL(join(core, 'constants.js')).href) as { defaultCodeSplitGroupings: unknown };
  return (code, file) => compilers.compileCodeSplitReferenceRoute({ code, codeSplitGroupings: constants.defaultCodeSplitGroupings, targetFramework: 'react', filename: file, id: file })?.code ?? code;
}

export function resolveImport(src: string, from: string, specifier: string): { file: string } | { pkg: string } | null {
  let base: string;
  if (specifier.startsWith('@/')) base = join(src, specifier.slice(2));
  else if (specifier.startsWith('.')) base = resolve(dirname(from), specifier);
  else return { pkg: specifier.startsWith('@') ? specifier.split('/').slice(0, 2).join('/') : specifier.split('/')[0] ?? specifier };
  for (const extension of EXTENSIONS) {
    const file = base + extension;
    if (existsSync(file) && statSync(file).isFile()) return { file };
  }
  return null;
}

/** The module specifiers a file imports at once: not the type-only ones, and not the dynamic ones. */
export function staticSpecifiers(file: string, code: string): string[] {
  if (!/\.tsx?$/.test(file)) return [];
  const source = ts.createSourceFile(file, code, ts.ScriptTarget.Latest, true, file.endsWith('x') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const found: string[] = [];
  for (const statement of source.statements) {
    if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
      const clause = statement.importClause;
      if (clause?.phaseModifier === ts.SyntaxKind.TypeKeyword) continue;
      const named = clause?.namedBindings;
      if (clause !== undefined && clause.name === undefined && named !== undefined && ts.isNamedImports(named) && named.elements.length > 0 && named.elements.every((element) => element.isTypeOnly)) continue;
      found.push(statement.moduleSpecifier.text);
    } else if (ts.isExportDeclaration(statement) && statement.moduleSpecifier !== undefined && ts.isStringLiteral(statement.moduleSpecifier) && !statement.isTypeOnly) {
      found.push(statement.moduleSpecifier.text);
    }
  }
  return found;
}

/** Walks the static imports from `entry` (a path under the frontend directory, `src/main.tsx`). */
export async function entryGraph(entry: string, root = process.cwd(), splitRoutes = true): Promise<EntryGraph> {
  const src = join(root, 'src');
  const split = splitRoutes ? await routeSplitter(root) : (code: string) => code;
  const start = join(root, entry);
  const modules = new Map<string, string | null>([[relative(src, start), null]]);
  const packages = new Map<string, string>();
  const queue = [start];
  let bytes = 0;
  for (let file = queue.shift(); file !== undefined; file = queue.shift()) {
    bytes += statSync(file).size;
    let code = readFileSync(file, 'utf8');
    const isRoute = file.includes(`${join(src, 'routes')}/`) && code.includes('createFileRoute(') && !basename(file).startsWith('-');
    if (isRoute) code = split(code, file);
    for (const specifier of staticSpecifiers(file, code)) {
      const target = resolveImport(src, file, specifier);
      if (target === null) continue;
      if ('pkg' in target) {
        if (!packages.has(target.pkg)) packages.set(target.pkg, relative(src, file));
        continue;
      }
      const name = relative(src, target.file);
      if (modules.has(name)) continue;
      modules.set(name, relative(src, file));
      queue.push(target.file);
    }
  }
  return { modules, packages, bytes };
}

/** The way from the entry to a module, as the names of the modules in between. */
export function wayTo(graph: EntryGraph, module: string): string[] {
  const way: string[] = [];
  for (let at: string | null | undefined = module; at !== null && at !== undefined; at = graph.modules.get(at)) way.unshift(at);
  return way;
}
