// @vitest-environment node
/** Every declared page can be located and loaded by the workbench that draws it. */
import { readFileSync, readdirSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import ts from 'typescript';
import { expect, it } from 'vitest';
import { locatePage, PAGE_ROUTES } from '@/features/workbench/pageTargets';

const sourceRoot = resolve(import.meta.dirname, '../../src');
function files(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    return entry.isDirectory() ? files(path) : path.endsWith('.tsx') ? [path] : [];
  });
}
function parse(path: string): ts.SourceFile {
  return ts.createSourceFile(path, readFileSync(path, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
}
function declaredPages(source: ts.SourceFile): string[] {
  const paths: string[] = [];
  const visit = (node: ts.Node) => {
    if (ts.isCallExpression(node) && ts.isCallExpression(node.expression)
      && ts.isIdentifier(node.expression.expression) && node.expression.expression.text === 'createFileRoute') {
      const path = node.expression.arguments[0], options = node.arguments[0];
      if (path !== undefined && ts.isStringLiteralLike(path) && options !== undefined && ts.isObjectLiteralExpression(options)) {
        const data = options.properties.find((entry) => ts.isPropertyAssignment(entry) && entry.name.getText(source) === 'staticData');
        if (data !== undefined && ts.isPropertyAssignment(data) && ts.isObjectLiteralExpression(data.initializer)
          && data.initializer.properties.some((entry) => ts.isPropertyAssignment(entry) && entry.name.getText(source) === 'screen'
            && ts.isStringLiteralLike(entry.initializer) && entry.initializer.text === 'page')) paths.push(path.text);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return paths;
}
function loaderPaths(source: ts.SourceFile): Set<string> {
  const paths = new Set<string>();
  const visit = (node: ts.Node) => {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.name.text === 'pages' && node.initializer !== undefined) {
      let value = node.initializer;
      while (ts.isAsExpression(value) || ts.isSatisfiesExpression(value) || ts.isParenthesizedExpression(value)) value = value.expression;
      if (!ts.isObjectLiteralExpression(value)) throw new Error('The page loaders must be enumerable.');
      for (const entry of value.properties) if (ts.isPropertyAssignment(entry) && ts.isStringLiteralLike(entry.name)) paths.add(entry.name.text);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return paths;
}

it('registers every route marked screen page in both the locator and component loader', () => {
  const loaders = loaderPaths(parse(resolve(sourceRoot, 'features/workbench/pages.ts')));
  const missing: string[] = [];
  let checked = 0;
  for (const file of files(resolve(sourceRoot, 'routes'))) for (const path of declaredPages(parse(file))) {
    checked += 1;
    if (!PAGE_ROUTES.some((route) => route === path)) missing.push(`${relative(sourceRoot, file)}: missing locator for ${path}`);
    if (!loaders.has(path)) missing.push(`${relative(sourceRoot, file)}: missing loader for ${path}`);
  }
  expect(checked).toBeGreaterThan(0);
  expect(missing).toEqual([]);
  expect([...loaders].sort()).toEqual([...PAGE_ROUTES].sort());
});

it('locates the import report address used by Settings and the finished job chip', () => {
  expect(locatePage('/settings/imports')).toEqual({ route: '/settings/imports/', pathname: '/settings/imports', params: {} });
});
