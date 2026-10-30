/** Choosing an existing program or an address, with write-only authorization and explicit consent for that address to receive a key. */
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { DeclaredHint } from '@/features/resources/Hint';
import type { Field } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { FieldForm, initialValues, missingFields, type FormValues } from '@/ui/FieldForm';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { resourcePreview } from '@/features/resource-facts/ResourceFacts';
import { ResourceSentence } from '@/features/resource-facts/ResourceSentence';
import type { ResourceSubject } from '@/features/resource-facts/facts';

/** Editable form state, never the stored source or a wire body. */
export interface SourceDraft {
  by: 'default' | 'program' | 'address' | 'none';
  program: string;
  url: string;
  auth: 'none' | 'bearer' | 'header';
  header: string;
  authorization: string;
  sendCredential: boolean;
  fields: FormValues;
}
/** A program already offered at the keeper, projected from setup/runtime declarations. No command is accepted from this form. */
export interface SourceProgram {
  name: string;
  choice?: string;
  title: string;
  summary: string;
  reports: readonly string[] | null;
  fields: readonly Field[];
  command: readonly string[] | null;
  templateId: string | null;
  templateName?: string | null;
}
export interface SourceFieldsProps {
  /** The server's current refusal for this source element, in its own words. */
  serverError?: string;
  serverErrors?: Readonly<Record<string,string>>;
  element: 'facts' | 'models';
  value: SourceDraft;
  onChange: (value: SourceDraft) => void;
  programs: readonly SourceProgram[];
  defaultTitle: string;
  defaultSummary: string;
  httpAllowed: boolean | null;
  canSendCredential: boolean;
  /** No credential needs no native-sign-in warning; older callers retain the native refusal. */
  credentialKind?: 'key' | 'none' | 'native';
  disabled?: boolean;
  /** Creation uses short source names; field help stays behind its question mark. */
  compact?: boolean;
  savedAddress: { url: string; auth: SourceDraft['auth']; header: string } | null;
  authorizationSet: boolean;
  secretFieldsSet: readonly string[];
  addressFields?: readonly Field[];
  warnings?: readonly ReactNode[];
}

export function emptySourceDraft(): SourceDraft { return { by:'default',program:'',url:'',auth:'none',header:'',authorization:'',sendCredential:false,fields:{} }; }
export function secretSourceField(field:Field):boolean { return field.type === 'secret' || field.type === 'secret_text' || field.type === 'secret_map'; }
/** A declaration may provide public defaults; a secret declaration never supplies a value to redisplay. */
export function sourceValues(fields:readonly Field[], values:FormValues = {}):FormValues {
  const initial=initialValues(fields);
  for (const field of fields) if (secretSourceField(field)) initial[field.name]=field.type === 'secret_map' ? {} : '';
  return {...initial,...values};
}
export function sourceHost(address: string): string | null {
  try { const url = new URL(address); return (url.protocol === 'https:' || url.protocol === 'http:') && url.username === '' && url.password === '' && url.hash === '' ? url.host : null; }
  catch { return null; }
}
export function sourceUnencrypted(address:string):boolean { try { return new URL(address.trim()).protocol === 'http:'; } catch { return false; } }
/** A saved token can be kept only for the unchanged address, mode and header it was given for. */
export function keepsAuthorization(value: SourceDraft, saved: SourceFieldsProps['savedAddress'], set: boolean): boolean {
  return set && saved !== null && value.url.trim() === saved.url && value.auth === saved.auth && value.header.trim() === saved.header;
}
export function sourceFault(value: SourceDraft, context: Pick<SourceFieldsProps,'programs' | 'httpAllowed' | 'canSendCredential' | 'savedAddress' | 'authorizationSet'> & Partial<Pick<SourceFieldsProps,'secretFieldsSet' | 'addressFields'>>): 'program' | 'url' | 'http' | 'header' | 'authorization' | 'credential' | 'fields' | null {
  const program=context.programs.find((program) => (program.choice ?? program.name) === value.program);
  const fields=value.by === 'program' ? program?.fields ?? [] : value.by === 'address' ? context.addressFields ?? [] : [];
  const missing=missingFields(fields,sourceValues(fields,value.fields)).filter((name) => !(context.secretFieldsSet ?? []).includes(name));
  if (value.by === 'program') return program === undefined ? 'program' : missing.length > 0 ? 'fields' : null;
  if (value.by !== 'address') return null;
  if (sourceHost(value.url.trim()) === null) return 'url';
  if (sourceUnencrypted(value.url) && context.httpAllowed !== true) return 'http';
  if (value.auth === 'header' && value.header.trim() === '') return 'header';
  if (value.auth !== 'none' && value.authorization === '' && !keepsAuthorization(value,context.savedAddress,context.authorizationSet)) return 'authorization';
  if (value.sendCredential && !context.canSendCredential) return 'credential';
  if (missing.length > 0) return 'fields';
  return null;
}

export function SourceFields({ element, value, onChange, programs, defaultTitle, defaultSummary, httpAllowed, canSendCredential, credentialKind, disabled = false, compact = false, savedAddress, authorizationSet, secretFieldsSet, addressFields = [], warnings = [], serverError, serverErrors = {} }: SourceFieldsProps) {
  const { t } = useTranslation();
  const [confirm, setConfirm] = useState<{ url:string; host:string } | null>(null);
  const program = programs.find((program) => (program.choice ?? program.name) === value.program);
  const declaredErrors=Object.fromEntries<string>(Object.entries(serverErrors).flatMap(([key,message]):[string,string][]=>{
    const field=/^(?:config|secrets)\.(.+)$/.exec(key)?.[1];return field===undefined?[]:[[field,message]];
  }));
  const addressErrors={url:serverErrors['address.url'],header:serverErrors['address.header'],auth:serverErrors['address.auth']};
  const known=new Set(['name','address.url','address.header','address.auth','authorization']);
  const declared=program?.fields ?? addressFields;
  const unplaced=Object.entries(serverErrors).filter(([key])=>!known.has(key) && !declared.some((field)=>key==='config.'+field.name || key==='secrets.'+field.name)).map(([,message])=>message);
  const groupError=serverError ?? (unplaced.length===0?undefined:unplaced.join(' '));
  const reportedMembers = program?.reports?.map((member) => t(`providerDetails.members.${member}`, { defaultValue: member })).join(', ') ?? '';
  const host = sourceHost(value.url.trim());
  const unencrypted = sourceUnencrypted(value.url);
  const kept = keepsAuthorization(value,savedAddress,authorizationSet);
  const fault=sourceFault(value,{programs,httpAllowed,canSendCredential,savedAddress,authorizationSet,secretFieldsSet,addressFields});
  const change = (patch: Partial<SourceDraft>) => onChange({ ...value,...patch });
  const fieldHelp = (field: Field) => {
    const saved = secretFieldsSet.includes(field.name);
    if (!saved && (field.help === null || field.help.trim() === '')) return null;
    return <span className="inline-flex items-center gap-1">
      {saved && <span className="text-xs text-text-muted">{t('providerSources.setReplace')}</span>}
      <DeclaredHint term={field.label} help={field.help} />
    </span>;
  };
  return <fieldset disabled={disabled} className="flex min-w-0 flex-col gap-3">
    <legend className="text-sm font-medium">{t(element === 'facts' ? 'providerSources.detailsFrom' : 'providerSources.modelsFrom')}</legend>
    <Select<SourceDraft['by']> hideLabel error={groupError} label={t(element === 'facts' ? 'providerSources.detailsFrom' : 'providerSources.modelsFrom')} valueLabel={value.by === 'default' ? compact?defaultTitle:t('providerSources.default',{source:defaultTitle}) : undefined} value={value.by} onValueChange={(by) => change({by,authorization:'',sendCredential:false,fields:{}})} options={(compact?[
      {value:'default' as const,label:defaultTitle},
      ...(['program','address','none'] as const).map((by)=>({value:by,label:t(`choiceCatalog.sourceKind.${by==='none'&&element==='models'?'manual':by}.name`)})),
    ]:[
      {value:'default',...choiceOption(t,'sourceKind.default'),description:`${t('providerSources.default',{source:defaultTitle})}. ${defaultSummary}`},
      {value:'program',...choiceOption(t,'sourceKind.program')}, {value:'address',...choiceOption(t,'sourceKind.address')},
      {value:'none',...choiceOption(t,element === 'facts' ? 'sourceKind.none' : 'sourceKind.manual')},
    ])} />
    {value.by === 'program' && <>
      <Select label={t('providerSources.registeredProgram')} error={serverErrors.name} value={value.program} onValueChange={(name) => change({program:name,fields:sourceValues(programs.find((program) => (program.choice ?? program.name) === name)?.fields ?? [])})} options={programs.map((program) => {
        const label = program.templateName === null || program.templateName === undefined ? program.title : t('providerSources.inTemplate', { program: program.title, template: program.templateName });
        const choice = { value: program.choice ?? program.name, label, description: program.summary };
        if (program.templateId === null || program.templateId === '' || program.templateName === null || program.templateName === undefined || program.templateName === '') return choice;
        const subject: ResourceSubject = { kind: 'template', template: { id: program.templateId, name: program.templateName } };
        const preview = resourcePreview(subject);
        return { ...choice, fullLabel: label, name: <ResourceSentence text={label} resources={[{ label: program.templateName, subject }]} card={false} />, preview: { label: preview.label, content: preview.content, hint: preview.hint } };
      })} placeholder={t('providerSources.chooseProgram')} />
      {program === undefined ? <InlineAlert tone="info">{t('providerSources.programUnavailable')}</InlineAlert> : <>
        <p className="text-sm text-text-muted">{program.reports === null ? t('providerSources.reportsUnread') : element === 'models' ? t('providerSources.listsModels') : program.reports.length === 0 ? t('providerSources.reportsNone') : t('providerSources.reports',{members:reportedMembers})}</p>
        {program.command !== null && <p className="break-words text-sm"><span>{t('providerSources.registeredCommand')}{': '}</span><code>{program.command.join(' ')}</code></p>}
        {program.templateId !== null && <InlineAlert tone="info">{t('providerSources.templateProgram')}</InlineAlert>}
        <FieldForm fields={program.fields} values={sourceValues(program.fields,value.fields)} errors={declaredErrors} helpFor={fieldHelp} onChange={(name,next) => change({fields:{...value.fields,[name]:next}})} />
      </>}
    </>}
    {value.by === 'address' && <>
      <Input type="url" label={t('providerSources.url')} error={addressErrors.url} value={value.url} autoComplete="off" onChange={(event) => change({url:event.target.value,sendCredential:false,authorization:'',fields:{}})} />
      {unencrypted && <Badge tone="warning">{t('providerSources.unencrypted')}</Badge>}
      {unencrypted && httpAllowed !== true && <InlineAlert tone="warning">{t(httpAllowed === null ? 'providerSources.httpUnknown' : 'providerSources.httpOff')}</InlineAlert>}
      <Select<SourceDraft['auth']> error={addressErrors.auth} label={t('providerSources.authorization')} value={value.auth} onValueChange={(auth) => change({auth,authorization:''})} options={[{value:'none',...choiceOption(t,'sourceAuthorization.none')},{value:'bearer',...choiceOption(t,'sourceAuthorization.bearer')},{value:'header',...choiceOption(t,'sourceAuthorization.header')}]} />
      {value.auth === 'header' && <Input label={t('providerSources.header')} error={addressErrors.header} value={value.header} autoComplete="off" onChange={(event) => change({header:event.target.value,authorization:''})} />}
      {value.auth !== 'none' && <PasswordInput label={t('providerSources.secret')} error={serverErrors.authorization} value={value.authorization} autoComplete="new-password" hint={t(kept ? 'providerSources.setReplace' : 'providerSources.writeOnly')} onChange={(event) => change({authorization:event.target.value})} />}
      {savedAddress !== null && savedAddress.url === value.url.trim() && addressFields.length > 0 && <FieldForm fields={addressFields} values={sourceValues(addressFields,value.fields)} errors={declaredErrors} helpFor={fieldHelp} onChange={(name,next) => change({fields:{...value.fields,[name]:next}})} />}
      {canSendCredential ? <Checkbox label={t('providerSources.sendKey')} checked={value.sendCredential} disabled={host === null || disabled} onCheckedChange={(checked) => { if (!checked) change({sendCredential:false}); else if (host !== null) setConfirm({url:value.url.trim(),host}); }} /> : credentialKind === 'none' ? null : <p className="text-sm text-text-muted">{t('providerSources.noSignIn')}</p>}
      {value.sendCredential && host !== null && <InlineAlert tone="warning">{t('providerSources.keyWillBeSent',{host})}</InlineAlert>}
      {warnings.map((warning,index) => <InlineAlert key={index} tone="warning">{warning}</InlineAlert>)}
    </>}
    {value.by === 'none' && <p className="text-sm text-text-muted">{t(element === 'facts' ? 'providerSources.nothingAbout' : 'providerSources.manualAbout')}</p>}
    {fault !== null && <InlineAlert tone="warning">{t(`providerSources.faults.${fault}`)}</InlineAlert>}
    <ConfirmDialog open={confirm !== null} onOpenChange={(open) => { if (!open) setConfirm(null); }} title={t('providerSources.confirmTitle',{host:confirm?.host ?? ''})} confirmLabel={t('providerSources.confirm')} danger onConfirm={() => { if (confirm !== null && confirm.url === value.url.trim()) change({sendCredential:true}); setConfirm(null); }}>
      <p>{t('providerSources.confirmAbout',{host:confirm?.host ?? ''})}</p>
    </ConfirmDialog>
  </fieldset>;
}
