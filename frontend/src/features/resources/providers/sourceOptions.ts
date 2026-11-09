/** Projects the server's offers into source controls and writes only the contract's choices. */
import type { Field, ProviderSetupSource, SourceChoiceSet } from '@/generated/api-types';
import { textValues } from '@/ui/FieldForm';
import { secretSourceField, sourceValues, type SourceDraft, type SourceProgram } from './SourceFields';

export type SourceElement = 'facts' | 'models';

export function localizedSourceSummary(descriptor: unknown, fallback: string, language: string): string {
  if (typeof descriptor !== 'object' || descriptor === null || !('summary_locales' in descriptor)) return fallback;
  const localized = descriptor.summary_locales;
  if (typeof localized !== 'object' || localized === null) return fallback;
  const values: Record<string, unknown> = {...localized};
  const value = values[language] ?? values[language.split('-')[0] ?? 'en'] ?? values.en;
  return typeof value === 'string' ? value : fallback;
}

/** Labels belong to the source declaration; a new provider needs no screen-specific vocabulary. */
export function localizedSourceFields(descriptor: unknown, fields: readonly Field[], language: string): readonly Field[] {
  if (typeof descriptor !== 'object' || descriptor === null || !('facts' in descriptor)) return fields;
  const facts = descriptor.facts;
  if (typeof facts !== 'object' || facts === null || !('admin_credential' in facts)) return fields;
  const admin = facts.admin_credential;
  if (typeof admin !== 'object' || admin === null || !('labels' in admin)) return fields;
  const labels = admin.labels;
  if (typeof labels !== 'object' || labels === null) return fields;
  const values: Record<string, unknown> = { ...labels };
  const label = values[language] ?? values[language.split('-')[0] ?? 'en'] ?? values.en;
  return typeof label === 'string' ? fields.map((field) => field.name === 'admin_key' ? {...field,label} : field) : fields;
}

/** A program's UI identity includes its template, since two templates may register the same local name. */
export function programChoice(program: SourceProgram): string { return program.choice ?? program.name; }
export function selectedProgram(value: SourceDraft, programs: readonly SourceProgram[]): SourceProgram | undefined {
  return value.by === 'program' ? programs.find((program) => programChoice(program) === value.program) : undefined;
}

/** Only the keeper's programs are offered; creation may also choose a template that carries one. */
export function programOffers(offers: readonly ProviderSetupSource[], element: SourceElement, machineId: string | null, templateId: string | null, chooseTemplate = false, language = 'en'): SourceProgram[] {
  return offers.filter((offer) => {
    const machine=offer.machine?.id,template=offer.template?.id;
    return offer.by === 'program' && (machine === machineId || machineId === null && template !== undefined && (chooseTemplate || template === templateId))
      && offer.descriptor?.[element] !== null;
  })
    .map((offer) => ({
      name:offer.name, choice:chooseTemplate && offer.template !== null ? `${offer.template.id}:${offer.name}` : offer.name,
      title:offer.descriptor?.title ?? offer.name, summary:localizedSourceSummary(offer.descriptor,offer.descriptor?.summary ?? '',language),
      reports:offer.descriptor === null ? null : element === 'facts' ? offer.descriptor.facts?.reports ?? [] : [],
      fields:localizedSourceFields(offer.descriptor,offer.descriptor?.fields ?? [],language), command:offer.command, templateId:offer.template?.id ?? null,
      templateName:offer.template?.name ?? null,
    }));
}

/** Hosts and descriptors are served metadata: no vendor names or inferred support live in the screen. */
export function defaultOffer(offers: readonly ProviderSetupSource[], element: SourceElement, endpoints: readonly {base_url:string}[], adapter: boolean): ProviderSetupSource | undefined {
  const builtins=offers.filter((offer) => offer.by === 'builtin');
  if (adapter) return builtins.find((offer) => offer.default_for.length === 0 && offer.descriptor !== null && offer.descriptor.facts !== null);
  if (element === 'models') return builtins.find((offer) => offer.descriptor !== null && offer.descriptor.models !== null && offer.descriptor.facts === null);
  for (const endpoint of endpoints) {
    let host:string;
    try {host=new URL(endpoint.base_url).hostname.toLowerCase();} catch {continue;}
    const offer=builtins.find((candidate) => candidate.default_for.some((allowed) => allowed.toLowerCase() === host));
    if (offer !== undefined) return offer;
  }
  return undefined;
}

/** Public field values stay typed; a secret field is written separately, with empty meaning keep. */
export function sourceChoiceSet(value: SourceDraft, fields: readonly Field[], programs: readonly SourceProgram[]): SourceChoiceSet {
  const choice:SourceChoiceSet={by:value.by};
  if (value.by === 'none') return choice;
  const config:Record<string,unknown>={};
  const secrets:Record<string,string|null>={};
  const values=sourceValues(fields,value.fields);
  const secretText=textValues(fields.filter(secretSourceField),values);
  for (const field of fields) {
    // Default source selection inherits untouched fields; an explicit false, empty value or
    // secret removal still belongs to the person's draft and must survive the write.
    if (value.by === 'default' && !Object.hasOwn(value.fields,field.name) && !value.removedSecrets?.includes(field.name)) continue;
    if (secretSourceField(field)) {
      const text=secretText[field.name];
      if (text !== undefined && text !== '') secrets[field.name]=text;
      else if (value.removedSecrets?.includes(field.name)) secrets[field.name]=null;
    } else if (values[field.name] !== undefined) config[field.name]=values[field.name];
  }
  if (value.by === 'default') {
    if (Object.keys(config).length>0) choice.config=config;
    if (Object.keys(secrets).length>0) choice.secrets=secrets;
    return choice;
  }
  choice.config=config;choice.secrets=secrets;
  if (value.by === 'builtin') choice.name=value.program;
  else if (value.by === 'program') choice.name=selectedProgram(value,programs)?.name;
  else {
    choice.address={url:value.url.trim(),auth:value.auth,send_credential:value.sendCredential,...(value.auth === 'header' ? {header:value.header.trim()} : {})};
    if (value.auth === 'none') choice.authorization=null;
    else if (value.authorization !== '') choice.authorization=value.authorization;
  }
  return choice;
}
