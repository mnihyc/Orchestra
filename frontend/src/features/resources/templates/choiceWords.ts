/** Driver-owned names and descriptions remain safe when a catalogue has no text for a new choice. */
import { useTranslation } from 'react-i18next';
import { record } from '@/features/resources/UnitCommands';
import type { ChoiceCopy } from '@/i18n/choices';
import { displayValue, fieldPath, type Schema } from '@/features/resources/templates/schema';

export function useTemplateChoiceWords(driver = 'docker') {
  const { t, i18n } = useTranslation();
  return (parts: readonly string[], value: unknown, schema: Schema = {}): ChoiceCopy => {
    const key = displayValue(value);
    const declared = record(record(schema['x-choices'])[key]);
    const fallbackName = typeof declared.name === 'string' && declared.name!=='' ? declared.name : key===''?'""':key;
    const fallbackDescription = typeof declared.description === 'string' ? declared.description : typeof schema.description === 'string' ? schema.description : '';
    const prefix = `templates.choices.${fieldPath(parts)}.${key}`;
    const nameKey = `${prefix}.name`;
    const descriptionKey = `${prefix}.description`;
    const known = i18n.exists(`templates.driverChoices.${driver}.name`) || i18n.exists(`templates.driverFields.${driver}.description`);
    return {
      name: known && i18n.exists(nameKey) ? t(nameKey, { defaultValue: fallbackName }) : fallbackName,
      description: known && i18n.exists(descriptionKey) ? t(descriptionKey, { defaultValue: fallbackDescription }) : fallbackDescription,
    };
  };
}
