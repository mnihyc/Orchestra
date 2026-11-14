/** A registered configuration uses the existing value controls inside the setting's draft. */
import { isRecord } from './draftChecks';
import { shapeOf, words } from './settingsModel';
import type { SummarizerForm } from './SummarizerForms';
import { ValueControl, type ValueControlProps } from './ValueControl';

export function SummarizerFields({ form, ...props }: ValueControlProps & { form: SummarizerForm }) {
  const held = isRecord(props.value) ? props.value : {};
  const configuration = Object.fromEntries(Object.entries(held).filter(([name]) => name !== 'kind'));
  const properties = isRecord(form.config_schema) && isRecord(form.config_schema.properties) ? form.config_schema.properties : {};
  return <ValueControl {...props} shape={shapeOf(form.config_schema)} value={configuration} hideLabel
    memberLabel={(name) => { const field = properties[name]; return isRecord(field) && typeof field.title === 'string' ? field.title : words(name); }}
    onChange={(next) => props.onChange({ ...(isRecord(next) ? next : {}), kind: form.name })} />;
}
