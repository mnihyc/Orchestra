/** Projects the server's offers into source controls and writes only the contract's choices. */
import type { Field, ProviderSetupSource, SourceChoiceSet } from '@/generated/api-types';
import { textValues } from '@/ui/FieldForm';
import { secretSourceField, sourceValues, type SourceDraft, type SourceProgram } from './SourceFields';

export type SourceElement = 'facts' | 'models';

/** A program's UI identity includes its template, since two templates may register the same local name. */
export function programChoice(program: SourceProgram): string { return program.choice ?? program.name; }
export function selectedProgram(value: SourceDraft, programs: readonly SourceProgram[]): SourceProgram | undefined {
  return value.by === 'program' ? programs.find((program) => programChoice(program) === value.program) : undefined;
}

/** Only the keeper's programs are offered; creation may also choose a template that carries one. */
export function programOffers(offers: readonly ProviderSetupSource[], element: SourceElement, machineId: string | null, templateId: string | null, chooseTemplate = false): SourceProgram[] {
  return offers.filter((offer) => {
    const machine=offer.machine?.id,template=offer.template?.id;
    return offer.by === 'program' && (machine === machineId || machineId === null && template !== undefined && (chooseTemplate || template === templateId))
      && offer.descriptor?.[element] !== null;
  })
    .map((offer) => ({
      name:offer.name, choice:chooseTemplate && offer.template !== null ? `${offer.template.id}:${offer.name}` : offer.name,
      title:offer.descriptor?.title ?? offer.name, summary:offer.descriptor?.summary ?? '',
      reports:offer.descriptor === null ? null : element === 'facts' ? offer.descriptor.facts?.reports ?? [] : [],
      fields:offer.descriptor?.fields ?? [], command:offer.command, templateId:offer.template?.id ?? null,
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
  if (value.by === 'default' || value.by === 'none') return choice;
  const config:Record<string,unknown>={};
  const secrets:Record<string,string>={};
  const values=sourceValues(fields,value.fields);
  const secretText=textValues(fields.filter(secretSourceField),values);
  for (const field of fields) {
    if (secretSourceField(field)) {
      const text=secretText[field.name];
      if (text !== undefined) secrets[field.name]=text;
    } else if (values[field.name] !== undefined) config[field.name]=values[field.name];
  }
  choice.config=config;choice.secrets=secrets;
  if (value.by === 'program') choice.name=selectedProgram(value,programs)?.name;
  else {
    choice.address={url:value.url.trim(),auth:value.auth,send_credential:value.sendCredential,...(value.auth === 'header' ? {header:value.header.trim()} : {})};
    if (value.auth === 'none') choice.authorization=null;
    else if (value.authorization !== '') choice.authorization=value.authorization;
  }
  return choice;
}
