/** Creation asks for an image or host account and required inputs; the complete editor stays on the page. */
import { useCallback, useRef } from 'react';
import { record } from '@/features/resources/UnitCommands';
import { SchemaDraftState, SchemaInput } from './SchemaForm';
import { pointerPart, replaceAt, resolveSchema, valueAt, type Schema, type SpecProblem } from './schema';

const PRIMARY_FIELDS = ['image', 'run_as'] as const;

/** Required primitive inputs remain available for plugin drivers with no worked example. */
function creationFields(root: Schema, given: unknown, value: unknown, parts: string[] = [], visited: ReadonlySet<unknown> = new Set()): { parts: string[]; schema: Schema }[] {
  const source = record(given);
  const reference = typeof source.$ref === 'string' ? source.$ref : given;
  const schema = resolveSchema(root, given, value);
  const properties = record(schema.properties);
  if (Object.keys(properties).length === 0 || visited.has(reference)) return [{ parts, schema }];
  const ancestors = new Set(visited); ancestors.add(reference);
  const required: unknown[] = Array.isArray(schema.required) ? schema.required : [];
  return Object.entries(properties).flatMap(([name, field]) => {
    if (!required.includes(name) && !(parts.length === 0 && PRIMARY_FIELDS.some((primary) => primary === name))) return [];
    const current = record(value)[name];
    const next = resolveSchema(root, field, current);
    const enoughItems = Array.isArray(current) && current.length >= (typeof next.minItems === 'number' ? next.minItems : 0);
    const opaqueObject = next.type === 'object' && Object.keys(record(next.properties)).length === 0 && current !== null && typeof current === 'object' && !Array.isArray(current)
      && Object.keys(current).length >= (typeof next.minProperties === 'number' ? next.minProperties : 0);
    if (next.type === 'array' && enoughItems || opaqueObject) return [];
    return creationFields(root, field, current, [...parts, name], ancestors);
  });
}

/** The dialog can place a server fault only at an input it actually renders. */
export function creationPointers(schema: Schema, spec: Record<string, unknown>): string[] {
  return creationFields(schema, schema, spec).filter((field) => field.parts.length > 0).map((field) => `/${field.parts.map(pointerPart).join('/')}`);
}

export function QuickTemplateFields({ schema, spec, driver, disabled = false, problems = [], onChange, onReady }: { schema: Schema; spec: Record<string, unknown>; driver: string; disabled?: boolean; problems?: readonly SpecProblem[]; onChange: (spec: Record<string, unknown>) => void; onReady: (ready: boolean) => void }) {
  const drafts = useRef(new Set<string>());
  const report = useCallback((id: string, invalid: boolean) => {
    if (invalid) drafts.current.add(id); else drafts.current.delete(id);
    onReady(drafts.current.size === 0);
  }, [onReady]);
  return <SchemaDraftState.Provider value={report}><div className="flex flex-col gap-3" data-template-creation-fields>
    {creationFields(schema, schema, spec).filter((field) => field.parts.length > 0).map((field) => <SchemaInput key={field.parts.map(pointerPart).join('/')} root={schema} schema={field.schema} value={valueAt(spec, field.parts)} parts={field.parts} driver={driver} disabled={disabled} embedded problems={problems} onChange={(next) => onChange(replaceAt(spec, field.parts, next))} />)}
  </div></SchemaDraftState.Provider>;
}
