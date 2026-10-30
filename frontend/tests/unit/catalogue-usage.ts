/** Read-only catalogue/source inventory for the ordinary-message regression; no product code is executed. */
import { readFileSync, readdirSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import ts from 'typescript';

export interface RuntimeMessages { source: string; keys: readonly string[]; reason: string }
interface SourceKeys { literals: Set<string>; patterns: Map<string, RegExp[]> }
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function messages(value: unknown, prefix = ''): Map<string, string> {
  const found = new Map<string, string>();
  if (!record(value)) return found;
  for (const [key, entry] of Object.entries(value)) {
    const path = prefix === '' ? key : `${prefix}.${key}`;
    if (typeof entry === 'string') found.set(path, entry);
    else for (const pair of messages(entry, path)) found.set(...pair);
  }
  return found;
}

const escape = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const pluralBase = (key: string): string => key.replace(/(?:_ordinal)?_(?:zero|one|two|few|many|other)$/u, '');

/** Resolve translated key expressions, including local prefix bindings. A workspace cache key or
 * filename template is not a translation use. Literal key dictionaries remain references too;
 * comments, JSX prose and test files do not keep a message alive. */
function sourceKeys(text: string, file: string): SourceKeys {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith('.tsx') ? ts.ScriptKind.TSX : file.endsWith('.json') ? ts.ScriptKind.JSON : ts.ScriptKind.TS);
  const literals = new Set<string>();
  const patterns = new Map<string, RegExp[]>();
  const bindings = new Map<string, ts.VariableDeclaration[]>();
  const collect = (node: ts.Node) => {
    if (ts.isStringLiteralLike(node)) literals.add(node.text);
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer !== undefined) {
      const previous = bindings.get(node.name.text) ?? [];
      bindings.set(node.name.text, [...previous, node]);
    }
    ts.forEachChild(node, collect);
  };
  collect(source);
  interface Form { pattern: string; prefix: string; exact: string | null }
  const unknown: Form = { pattern: '.+', prefix: '', exact: null };
  const fixed = (value: string): Form => ({ pattern: escape(value), prefix: value, exact: value });
  const append = (left: Form[], right: Form[]): Form[] => left.flatMap((one) => right.map((two) => ({
    pattern: one.pattern + two.pattern, prefix: one.exact === null ? one.prefix : one.prefix + two.prefix,
    exact: one.exact === null || two.exact === null ? null : one.exact + two.exact,
  })));
  const parameterForms = (type: ts.TypeNode | undefined): Form[] => {
    if (type === undefined) return [unknown];
    if (ts.isLiteralTypeNode(type) && ts.isStringLiteral(type.literal)) return [fixed(type.literal.text)];
    if (ts.isUnionTypeNode(type)) return type.types.flatMap(parameterForms);
    if (ts.isParenthesizedTypeNode(type)) return parameterForms(type.type);
    return [unknown];
  };
  const scopeOf = (node: ts.Node): ts.Node => {
    let at: ts.Node = node;
    while (!ts.isBlock(at) && !ts.isSourceFile(at)) at = at.parent;
    return at;
  };
  const forms = (node: ts.Expression, seen: ReadonlySet<ts.Node> = new Set()): Form[] => {
    if (seen.has(node)) return [unknown];
    const next = new Set(seen).add(node);
    if (ts.isStringLiteralLike(node)) return [fixed(node.text)];
    if (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isSatisfiesExpression(node) || ts.isTypeAssertionExpression(node)) return forms(node.expression, next);
    if (ts.isConditionalExpression(node)) return [...forms(node.whenTrue, next), ...forms(node.whenFalse, next)];
    if (ts.isArrayLiteralExpression(node)) return node.elements.flatMap((entry) => ts.isOmittedExpression(entry) ? [] : forms(entry, next));
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) return append(forms(node.left, next), forms(node.right, next));
    if (ts.isTemplateExpression(node)) return node.templateSpans.reduce((parts, span) => append(append(parts, forms(span.expression, next)), [fixed(span.literal.text)]), [fixed(node.head.text)]);
    if (ts.isIdentifier(node)) {
      for (let at = node.parent; ; at = at.parent) {
        if (ts.isFunctionLike(at)) {
          const parameter = at.parameters.find((one) => ts.isIdentifier(one.name) && one.name.text === node.text);
          if (parameter !== undefined) return parameterForms(parameter.type);
        }
        const declarations = (bindings.get(node.text) ?? []).filter((declaration) => scopeOf(declaration) === at && declaration.pos < node.pos);
        const declaration = declarations.at(-1);
        if (declaration?.initializer !== undefined) return forms(declaration.initializer, next);
        if (ts.isSourceFile(at)) break;
      }
    }
    return [unknown];
  };
  const visit = (node: ts.Node) => {
    if (ts.isCallExpression(node) && node.arguments[0] !== undefined) {
      const callee = node.expression;
      const translates = ts.isIdentifier(callee) ? /^t\d*$/u.test(callee.text) : ts.isPropertyAccessExpression(callee) && callee.name.text === 't';
      const reads = ts.isPropertyAccessExpression(callee) && callee.name.text === 'getResource' ? node.arguments[2]
        : translates || ts.isPropertyAccessExpression(callee) && callee.name.text === 'exists' ? node.arguments[0] : undefined;
      if (reads !== undefined) for (const form of forms(reads)) if (form.prefix.includes('.')) {
        if (form.exact !== null) {
          literals.add(form.exact);
          continue;
        }
        const root = form.prefix.split('.')[0] ?? '';
        const family = patterns.get(root) ?? [];
        family.push(new RegExp(`^${form.pattern}$`, 'u')); patterns.set(root, family);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return { literals, patterns };
}

export function productSources(root: string): Map<string, string> {
  const found = new Map<string, string>();
  const visit = (directory: string) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (/\.(?:tsx?|json)$/u.test(entry.name) && !entry.name.endsWith('.d.ts') && !['en.json', 'zh-CN.json', 'dev-en.json', 'dev-zh-CN.json'].includes(entry.name)) {
        found.set(relative(root, path).replaceAll('\\', '/'), readFileSync(path, 'utf8'));
      }
    }
  };
  visit(root);
  return found;
}

/** Parse readers once for both languages, indexed by root to avoid testing every pattern on every key. */
export function catalogueReaders(sources: ReadonlyMap<string, string>, runtime: readonly RuntimeMessages[]): SourceKeys {
  const literals = new Set<string>();
  const patterns = new Map<string, RegExp[]>();
  for (const [file, text] of sources) {
    const collected = sourceKeys(text, file);
    for (const key of collected.literals) literals.add(key);
    for (const [root, expressions] of collected.patterns) {
      const family = patterns.get(root) ?? [];
      family.push(...expressions); patterns.set(root, family);
    }
  }
  for (const declaration of runtime) if (sources.has(declaration.source)) for (const key of declaration.keys) literals.add(key);
  return { literals, patterns };
}

/** Resolve reached $t aliases to a fixed point, including an alias to a plural family. */
export function unusedMessages(catalogue: unknown, sources: ReadonlyMap<string, string>, runtime: readonly RuntimeMessages[], readers = catalogueReaders(sources, runtime)): string[] {
  const entries = messages(catalogue);
  const { literals, patterns } = readers;
  const used = new Set<string>();
  const names = [...entries.keys()];
  const families = new Map<string, string[]>();
  for (const key of names) {
    const base = pluralBase(key);
    const previous = families.get(base) ?? [];
    families.set(base, [...previous, key]);
    if (literals.has(key) || literals.has(base)) {
      used.add(key);
      continue;
    }
    const matches = (family: readonly RegExp[] | undefined) => family?.some((pattern) => pattern.test(key) || pattern.test(base)) === true;
    if (matches(patterns.get(key.split('.')[0] ?? '')) || matches(patterns.get(''))) used.add(key);
  }
  const pending = [...used];
  // Array iteration also visits aliases appended below; the used set ends cycles.
  for (const key of pending) {
    for (const match of (entries.get(key) ?? '').matchAll(/\$t\(([^),]+)/gu)) {
      const alias = match[1]?.trim();
      if (alias === undefined) continue;
      const targets = new Set([...(entries.has(alias) ? [alias] : []), ...(families.get(alias) ?? [])]);
      for (const target of targets) if (!used.has(target)) {
        used.add(target); pending.push(target);
      }
    }
  }
  return names.filter((key) => !used.has(key)).sort();
}

/** These promise absent product parts, rather than describing a person's empty/pending data. */
export const UNFINISHED_PRODUCT = /\b(?:not built|not yet (?:available|supported|possible|built|implemented)|this build|from this server|not served|available yet|supported yet|possible yet|part of this build|fill this section|does not take)\b|尚未(?:构建|开发|实现)|(?:本次|这个|此)构建|(?:此|该)服务器(?:暂时|暂不|尚不).*(?:提供|支持)|(?:功能|部分)(?:尚未|暂未)(?:实现|提供|开发)/iu;
