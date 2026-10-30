/** One configuration, a schema form and JSON view, checked by the same server rule that saves it. */
import { useQuery } from '@tanstack/react-query';
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { checkTemplate } from '@/api/resources';
import { ResourceDetails } from '@/features/resources/DetailSections';
import type { UnitKind } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Switch } from '@/ui/Switch';
import { JsonSpec } from './JsonSpec';
import { SchemaDraftState, SchemaTechnicalDetails } from './SchemaForm';
import { SchemaForm } from './SchemaSettings';
import { pointerParts, schemaAt, valueAt, type Schema, type SpecProblem } from './schema';
import { TemplateExamples } from './TemplateExamples';
import { readSpec, specText } from './spec';

/** Wait for a short typing pause so ordinary edits do not send a request per keystroke. */
const CHECK_PAUSE_MS = 350;

export function TemplateEditor({ schema, driver, kind, text, disabled = false, readOnly = false, examples = false, active = true, compact = false, leading, savedProblems = [], onChange, onValidityChange }: { schema: Schema; driver: string; kind: UnitKind; text: string; disabled?: boolean; readOnly?: boolean; examples?: boolean; active?: boolean; compact?: boolean; leading?: (query: string) => ReactNode; savedProblems?: readonly SpecProblem[]; onChange: (text: string) => void; onValidityChange?: (valid: boolean) => void }) {
  const { t } = useTranslation();
  const [json, setJson] = useState(false);
  const [waiting, setWaiting] = useState(text);
  const drafts = useRef(new Set<string>());
  const [invalidDrafts, setInvalidDrafts] = useState(false);
  const reportDraft = useCallback((id: string, invalid: boolean) => {
    if (invalid) drafts.current.add(id); else drafts.current.delete(id);
    const anyInvalid = drafts.current.size > 0;
    setInvalidDrafts(anyInvalid);
  }, []);
  useEffect(() => {
    if (!active) return;
    const timer = setTimeout(() => setWaiting(text), CHECK_PAUSE_MS);
    return () => clearTimeout(timer);
  }, [text, active]);
  const parsed = readSpec(text);
  const check = useQuery({ queryKey: ['template-check', driver, kind, schema, waiting], enabled: active && !readOnly && parsed !== null && text === waiting,
    queryFn: ({ signal }) => checkTemplate({ driver, kind, spec: readSpec(waiting) }, signal), retry: false, staleTime: Number.POSITIVE_INFINITY });
  const checked = parsed === null ? [{ pointer: '', message: t('templates.editor.invalidJson') }] : text === waiting ? check.data?.errors ?? [] : [];
  const problems: SpecProblem[] = [...new Map([...checked, ...savedProblems].map((problem) => [JSON.stringify([problem.pointer, problem.message]), problem])).values()];
  const valid = !invalidDrafts && parsed !== null && problems.length === 0;
  useEffect(() => { onValidityChange?.(valid); }, [valid, onValidityChange]);
  const generalProblems = problems.filter((problem) => problem.pointer === '' || Object.keys(schemaAt(schema, pointerParts(problem.pointer), parsed)).length === 0 && valueAt(parsed, pointerParts(problem.pointer)) === undefined);
  const toggle = <Switch name={t('templates.modeChoices.json.name')} description={t('templates.modeChoices.json.description')} checked={json} disabled={parsed === null && json || invalidDrafts} onCheckedChange={setJson} />;
  const jsonEditor = <>{toggle}{leading?.('')}<JsonSpec schema={schema} driver={driver} text={text} problems={problems} disabled={disabled || readOnly} onChange={onChange} /></>;
  return <div className="flex min-w-0 flex-col gap-4">
    {examples && <TemplateExamples sources={[{ driver, name: driver, schema }]} disabled={disabled || invalidDrafts} onChoose={(_driver, next) => onChange(specText(next))} />}
    {json ? compact ? <ResourceDetails key="json" defaultOpen keepMounted pauseEffects={false}>{jsonEditor}</ResourceDetails> : jsonEditor
      : parsed !== null ? <SchemaDraftState.Provider value={reportDraft}><SchemaTechnicalDetails value={compact}><SchemaForm schema={schema} spec={parsed} driver={driver} compact={compact} blocked={invalidDrafts}
          toolbar={compact ? <ResourceDetails key="mode" keepMounted pauseEffects={false}>{toggle}</ResourceDetails> : toggle} leading={leading} disabled={disabled} readOnly={readOnly} problems={problems} onChange={(next) => onChange(specText(next))} /></SchemaTechnicalDetails></SchemaDraftState.Provider>
        : <><InlineAlert tone="warning">{t('templates.editor.invalidJson')}</InlineAlert><Button onClick={() => setJson(true)}>{t('templates.editor.openJson')}</Button></>}
    {parsed !== null && check.isError && text === waiting && <ErrorState compact error={check.error} onRetry={() => void check.refetch()} />}
    {!readOnly && parsed !== null && <p aria-live="polite" className="text-sm text-text-muted">{text !== waiting || check.isFetching ? t('templates.editor.checking') : check.data?.valid === true ? t('templates.editor.valid') : ''}</p>}
    {!json && parsed !== null && generalProblems.length > 0 && <ul aria-label={t('templates.editor.problems')} className="text-sm text-danger-text">{generalProblems.map((problem, index) => <li key={index}>{problem.message}</li>)}</ul>}
  </div>;
}
