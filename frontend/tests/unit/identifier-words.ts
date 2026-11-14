/** Finds the words of the product that would show a person something a machine made: a hash, an identifier, a
 * revision counter, a serial. Read-only: it reads the catalogue's placeholders and the arguments of `t(...)` calls in
 * the sources, and runs no product code. */
import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import ts from 'typescript';

/** What a value is called when it is a machine's string: `id`, `hash`, `digest`, `revision`, `serial` and their kin. */
const PLAIN = /^(?:ids?|uuid|hash|digest|checksum|sha\d*|fingerprint\w*|serial|revision|generation|snapshot\w*|build|commit|pid|slug|reference|ref|etag|unit_?name)$/iu;
const CAMEL = /^[a-z]+(?:Id|Hash|Digest|Revision|Serial|Fingerprint)$/u;
const SNAKE = /_(?:id|hash|digest|revision|serial|fingerprint)$/u;

/** Whether a placeholder or a property is named for a machine's string. */
export function isIdentifierName(name: string): boolean {
  return PLAIN.test(name) || CAMEL.test(name) || SNAKE.test(name);
}

/** Every leaf of a catalogue, by its dotted key. */
export function leaves(value: unknown, prefix = ''): Map<string, string> {
  const found = new Map<string, string>();
  if (typeof value === 'string') return found.set(prefix, value);
  if (typeof value === 'object' && value !== null) {
    for (const [key, inner] of Object.entries(value)) for (const [path, text] of leaves(inner, prefix === '' ? key : `${prefix}.${key}`)) found.set(path, text);
  }
  return found;
}

/** The identifier-named placeholders of one catalogue as `key {{name}}` lines, in key order. */
export function identifierPlaceholders(catalogue: unknown): string[] {
  const found: string[] = [];
  for (const [key, text] of leaves(catalogue)) {
    for (const match of text.matchAll(/\{\{\s*([\w.]+)/gu)) {
      const name = match[1] ?? '';
      if (isIdentifierName(name)) found.push(`${key} {{${name}}}`);
    }
  }
  return found.sort();
}

export const sourceFiles = (directory: string): string[] =>
  readdirSync(directory, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory()
      ? entry.name === 'generated' || entry.name === 'dev' ? [] : sourceFiles(resolve(directory, entry.name))
      : /\.tsx?$/u.test(entry.name) ? [resolve(directory, entry.name)] : []);

/** The name of the last step of a value: `row.unit_name` is `unit_name`; `data?.id` is `id`; `id` is `id`. A value that
 * is not a plain read of a property or a variable has none. */
function lastStep(expression: ts.Expression): string[] {
  if (ts.isParenthesizedExpression(expression) || ts.isNonNullExpression(expression) || ts.isAsExpression(expression)) return lastStep(expression.expression);
  if (ts.isIdentifier(expression)) return [expression.text];
  if (ts.isPropertyAccessExpression(expression)) return [expression.name.text];
  if (ts.isBinaryExpression(expression) && expression.operatorToken.kind === ts.SyntaxKind.QuestionQuestionToken) return [...lastStep(expression.left), ...lastStep(expression.right)];
  if (ts.isConditionalExpression(expression)) return [...lastStep(expression.whenTrue), ...lastStep(expression.whenFalse)];
  return [];
}

/** Calls of the translation function that hand an identifier-named value to a sentence, as `file: key (property)` lines.
 * The value counts when its own last step is an identifier name (`id: row.unit_name`) wherever the sentence names it. */
export function identifierArguments(root: string): string[] {
  const found: string[] = [];
  for (const file of sourceFiles(root)) {
    const text = readFileSync(file, 'utf8');
    const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith('x') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
    const visit = (node: ts.Node) => {
      if (ts.isCallExpression(node)) {
        const callee = node.expression;
        const named = ts.isIdentifier(callee) ? callee.text === 't' : ts.isPropertyAccessExpression(callee) && callee.name.text === 't';
        const [key, values] = node.arguments;
        if (named && key !== undefined && (ts.isStringLiteralLike(key) || ts.isTemplateExpression(key)) && values !== undefined && ts.isObjectLiteralExpression(values)) {
          const words = ts.isStringLiteralLike(key) ? key.text : key.head.text;
          for (const property of values.properties) {
            if (ts.isShorthandPropertyAssignment(property)) {
              if (isIdentifierName(property.name.text)) found.push(`${file.slice(root.length + 1)}: ${words} (${property.name.text})`);
            } else if (ts.isPropertyAssignment(property) && lastStep(property.initializer).some(isIdentifierName)) {
              found.push(`${file.slice(root.length + 1)}: ${words} (${property.name.getText(source)})`);
            }
          }
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return found.sort();
}
