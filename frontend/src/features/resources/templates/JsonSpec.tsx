/** JSON editing with driver help, property completion and the server's problems at their actual lines. */
import { autocompletion, completionKeymap } from '@codemirror/autocomplete';
import { json, jsonLanguage, jsonParseLinter } from '@codemirror/lang-json';
import { syntaxTree } from '@codemirror/language';
import { linter, lintGutter } from '@codemirror/lint';
import { hoverTooltip, keymap } from '@codemirror/view';
import { useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { record } from '@/features/resources/UnitCommands';
import { CodeView, type CodeViewHandle } from '@/ui/CodeView';
import { Button } from '@/ui/Button';
import { readSpec } from './spec';
import { pointerPart, resolveSchema, schemaAt, type Schema, type SpecProblem } from './schema';
import { useFieldWords } from './SchemaForm';

interface JsonRange { pointer: string; parts: string[]; from: number; to: number; object: boolean }
type JsonNode = ReturnType<typeof syntaxTree>['topNode'];

/** The JSON parser keeps repeated property names, escaped keys and array entries distinct. */
export function jsonRanges(text: string): JsonRange[] {
  const ranges: JsonRange[] = [];
  const visit = (node: JsonNode, parts: string[], keyFrom?: number) => {
    ranges.push({ pointer: parts.length === 0 ? '' : `/${parts.map(pointerPart).join('/')}`, parts, from: keyFrom ?? node.from, to: node.to, object: node.name === 'Object' });
    if (node.name === 'Object') {
      for (let child = node.firstChild; child !== null; child = child.nextSibling) {
        if (child.name !== 'Property' || child.firstChild === null || child.lastChild === null) continue;
        let key: unknown;
        try { key = JSON.parse(text.slice(child.firstChild.from, child.firstChild.to)); } catch { continue; }
        if (typeof key === 'string') visit(child.lastChild, [...parts, key], child.from);
      }
    } else if (node.name === 'Array') {
      let index = 0;
      for (let child = node.firstChild; child !== null; child = child.nextSibling) {
        if (['Object', 'Array', 'String', 'Number', 'True', 'False', 'Null'].includes(child.name)) visit(child, [...parts, String(index++)]);
      }
    } else if (node.name === 'JsonText' && node.firstChild !== null) visit(node.firstChild, parts);
  };
  // Editor states parse within a time budget and viewport. Diagnostics need every key's position.
  visit(jsonLanguage.parser.parse(text).topNode, []);
  return ranges;
}

export function problemRange(text: string, pointer: string): { from: number; to: number; line: number } {
  const ranges = jsonRanges(text);
  let found = ranges.find((range) => range.pointer === pointer);
  let parent = pointer;
  while (found === undefined && parent !== '') {
    parent = parent.slice(0, parent.lastIndexOf('/'));
    found = ranges.find((range) => range.pointer === parent);
  }
  const from = found?.from ?? 0;
  return { from, to: Math.max(from, found?.to ?? 0), line: text.slice(0, from).split('\n').length };
}

export function JsonSpec({ schema, driver, text, problems, disabled, onChange }: { schema: Schema; driver: string; text: string; problems: readonly SpecProblem[]; disabled: boolean; onChange: (text: string) => void }) {
  const { t, i18n } = useTranslation();
  const words = useFieldWords(driver);
  const editor = useRef<CodeViewHandle>(null);
  const language = useMemo(() => () => Promise.resolve([json(), lintGutter(),
    linter((view) => [...jsonParseLinter()(view).map((problem) => ({ ...problem, message: t('templates.editor.invalidJson') })), ...problems.map((problem) => ({ ...problemRange(view.state.doc.toString(), problem.pointer), severity: 'error' as const, message: problem.message }))]),
    hoverTooltip((view, position) => {
      const source = view.state.doc.toString();
      const at = jsonRanges(source).filter((range) => range.parts.length > 0 && position >= range.from && position <= range.to).sort((a, b) => b.parts.length - a.parts.length)[0];
      if (at === undefined) return null;
      const field = schemaAt(schema, at.parts, readSpec(source));
      const description = words(at.parts, field, 'description');
      if (description === '') return null;
      return { pos: at.from, end: at.to, create: () => {
        const dom = document.createElement('div'); dom.className = 'max-w-sm p-3 text-sm';
        const title = document.createElement('strong'); title.textContent = words(at.parts, field, 'title');
        const help = document.createElement('p'); help.textContent = description;
        dom.append(title, help); return { dom };
      } };
    }),
    autocompletion({ override: [(context) => {
      const source = context.state.doc.toString();
      const token = context.matchBefore(/"[^"\n]*"?/);
      const syntax = syntaxTree(context.state).resolveInner(context.pos, -1);
      const prefix = source.slice(0, token?.from ?? context.pos).trimEnd();
      if (syntax.name !== 'PropertyName' && !prefix.endsWith('{') && !prefix.endsWith(',')) return null;
      const at = jsonRanges(source).filter((range) => range.object && context.pos >= range.from && context.pos <= range.to).sort((a, b) => b.parts.length - a.parts.length)[0];
      const field = schemaAt(schema, at?.parts ?? [], readSpec(source));
      const alternatives = field.anyOf ?? field.oneOf;
      const properties = Object.fromEntries([...(Array.isArray(alternatives) ? alternatives.flatMap((variant) => Object.entries(record(resolveSchema(schema, variant).properties))) : []), ...Object.entries(record(field.properties))]);
      const hasColon = /^\s*"?\s*:/.test(source.slice(context.pos));
      return { from: token?.from ?? context.pos, to: source[context.pos] === '"' ? context.pos + 1 : context.pos,
        options: Object.entries(properties).map(([name, given]) => ({ label: name, type: 'property', detail: words([...(at?.parts ?? []), name], record(given), 'title'), info: words([...(at?.parts ?? []), name], record(given), 'description'), apply: `${JSON.stringify(name)}${hasColon ? '' : ': '}` })) };
    }] }), keymap.of(completionKeymap),
  ]), [schema, problems, words, t]);
  // Help and diagnostics are closures on this schema, locale and validation result.
  const languageKey = useMemo(() => JSON.stringify([schema, problems, i18n.resolvedLanguage]), [schema, problems, i18n.resolvedLanguage]);
  return <div className="flex min-w-0 flex-col gap-2">
    <div className="h-96 min-w-0 overflow-hidden rounded-lg border border-border"><CodeView ref={editor} text={text} version={text} label={t('templates.spec.json')} language={language} languageKey={languageKey} readOnly={disabled} onChange={onChange} wrap /></div>
    {problems.length > 0 && <ul aria-label={t('templates.editor.problems')} className="flex flex-col gap-1">{problems.map((problem, index) => {
      const { line } = problemRange(text, problem.pointer);
      return <li key={`${problem.pointer}:${index}`}><Button size="sm" variant="ghost" onClick={() => { editor.current?.goToLine(line); editor.current?.focus(); }}>{t('templates.editor.lineProblem', { line, message: problem.message })}</Button></li>;
    })}</ul>}
  </div>;
}
