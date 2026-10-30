/** Driver-owned worked examples choose their driver and fill the same configuration the form edits. */
import { useTranslation } from 'react-i18next';
import { Button } from '@/ui/Button';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { templateExamples, type Schema } from './schema';

export interface ExampleSource { driver: string; name: string; schema: Schema }

export function TemplateExamples({ sources, disabled = false, onChoose }: { sources: readonly ExampleSource[]; disabled?: boolean; onChoose: (driver: string, spec: Record<string, unknown>) => void }) {
  const { t, i18n } = useTranslation();
  const groups = sources.map((source) => ({ ...source, examples: templateExamples(source.schema) })).filter((source) => source.examples.length > 0);
  const words = (driver: string, example: string, member: 'name' | 'description', fallback: string): string => {
    const key = `templates.editor.examplesText.${example}.${member}`;
    return (i18n.exists(`templates.driverChoices.${driver}.name`) || i18n.exists(`templates.driverFields.${driver}.description`)) && i18n.exists(key) ? t(key, { defaultValue: fallback }) : fallback;
  };
  if (groups.length === 0) return null;
  const entries: MenuEntry[] = groups.flatMap((source) => [
    ...(groups.length > 1 ? [{ type: 'label' as const, id: source.driver, label: source.name }] : []),
    ...source.examples.map((example) => ({ id: `${source.driver}:${example.id}`, name: words(source.driver, example.id, 'name', example.name),
      description: words(source.driver, example.id, 'description', example.description), onSelect: () => onChoose(source.driver, structuredClone(example.spec)) })),
  ]);
  return <div className="self-start"><Menu label={t('templates.editor.examples')} entries={entries}
    trigger={<Button size="sm" disabled={disabled}>{t('templates.editor.examples')}</Button>} /></div>;
}
