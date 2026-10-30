/** The dialog that makes a template: for environments or for a provider's runtime, on a driver a host reports, with a spec that begins from what that driver's schema asks for and is checked by the server before anything is kept.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { driversQuery, createTemplate } from '@/api/resources';
import { ApiError } from '@/api/client';
import { HomePicker } from '@/features/resources/HomePicker';
import { specSchema } from '@/features/resources/templates/spec';
import { newValue, templateExamples, type SpecProblem } from '@/features/resources/templates/schema';
import { record } from '@/features/resources/UnitCommands';
import { creationPointers, QuickTemplateFields } from '@/features/resources/templates/QuickTemplateFields';
import { TemplateExamples } from '@/features/resources/templates/TemplateExamples';
import { TemplateHosts } from '@/features/resources/templates/TemplateHosts';
import { TemplateGuideLink } from '@/features/help/TemplateGuideLink';
import type { UnitKind } from '@/generated/api-types';
import { useActKey, useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

export function NewTemplateDialog({ onClose }: { onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const drivers = useQuery(driversQuery);
  const key = useActKey();
  const [kind, setKind] = useState<UnitKind>('environment');
  const [chosen, setChosen] = useState('');
  // What was typed into the spec; nothing until the person touches it, so that the starting point follows the driver and the kind.
  const [typed, setTyped] = useState<Record<string, unknown> | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [placed, setPlaced] = useState(false);
  const [validDraft, setValidDraft] = useState(true);
  const form = useForm({ name: '', home_id: '' }, { name: (value) => (value.trim() === '' ? t('templates.new.nameEmpty') : null), home_id: (value) => value === '' ? t('templateSettings.homeEmpty') : null });
  const formElement = useRef<HTMLFormElement>(null);
  const errorElement = useRef<HTMLDivElement>(null);

  const offered = (drivers.data ?? []).filter((driver) => specSchema(driver.descriptor, kind) !== null);
  const driver = offered.find((entry) => entry.name === chosen) ?? offered[0] ?? null;
  const schema = driver === null ? null : specSchema(driver.descriptor, kind);
  const spec = typed ?? templateExamples(schema)[0]?.spec ?? (schema === null ? {} : record(newValue(schema, schema)));
  const purposes = (['environment','provider'] as const).map((value)=>({value,name:t(`templates.purposeChoices.${value}.name`),description:t(`templates.purposeChoices.${value}.description`)}));
  const driverOptions=offered.map((entry)=>{
    const nameKey=`templates.driverChoices.${entry.name}.name`;
    const descriptionKey=`templates.driverChoices.${entry.name}.description`;
    const driverDescriptionKey=`templates.driverFields.${entry.name}.description`;
    const declared=record(entry.descriptor);
    const name=i18n.exists(nameKey)?t(nameKey,{defaultValue:entry.title}):entry.title;
    const fallback=typeof declared.description==='string'?declared.description:'';
    const description=i18n.exists(descriptionKey)?t(descriptionKey,{defaultValue:fallback}):i18n.exists(driverDescriptionKey)?t(driverDescriptionKey,{defaultValue:fallback}):fallback;
    return {value:entry.name,name,description:[description,t('templates.new.driverHosts',{count:entry.hosts})].filter((part)=>part!=='').join(' ')};
  });
  const exampleSources = offered.flatMap((entry) => {
    const reported = specSchema(entry.descriptor, kind);
    return reported === null ? [] : [{ driver: entry.name, name: driverOptions.find((option) => option.value === entry.name)?.name ?? entry.title, schema: reported }];
  });

  const make = useMutation({
    mutationFn: (values: { name: string; home_id: string }) => {
      if (driver === null) throw new Error('the template was not ready');
      const body = { name: values.name.trim(), description: '', driver: driver.name, home_id: values.home_id, kind, spec, access: 'only_me' as const };
      return createTemplate(body, key(body));
    },
    onSuccess: (template) => {
      toast({ tone: 'success', title: t('templates.new.done', { name: template.name }) });
      onClose();
      void navigate({ to: '/resources/templates/$id', params: { id: template.id } });
    },
    onError: (error) => {
      const inputFault = error instanceof ApiError && schema !== null && error.faults().some((fault) => creationPointers(schema, spec).some((pointer) => fault.path === `spec${pointer}` || fault.path.startsWith(`spec${pointer}/`)));
      setPlaced(form.refuse(error) || inputFault);
    },
  });

  const submit = form.submit((values) => {
    if (driver === null || values.home_id === '' || !validDraft) return;
    make.mutate(values);
  });
  const problems: SpecProblem[] = make.error instanceof ApiError ? make.error.faults().filter((fault) => fault.path.startsWith('spec')).map((fault) => ({ pointer: fault.path.slice(4), message: fault.message })) : [];
  const unplaced = make.error instanceof ApiError && make.error.faults().some((fault) => !Object.hasOwn(form.values, fault.path) && (schema === null || !creationPointers(schema, spec).some((pointer) => fault.path === `spec${pointer}` || fault.path.startsWith(`spec${pointer}/`))));
  useEffect(() => {
    if (make.error === null) return;
    const field = formElement.current?.querySelector<HTMLElement>('[aria-invalid="true"]') ?? errorElement.current;
    field?.scrollIntoView({ block: 'center' }); field?.focus();
  }, [make.error, placed]);

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t('templates.new.title')}
      dismissible={!make.isPending}
      footer={
        <>
          <Button disabled={make.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="new-template" pending={make.isPending} blocked={driver === null || form.values.home_id === '' || !validDraft}>
            {t('templates.new.submit')}
          </Button>
        </>
      }
    >
      <form ref={formElement} id="new-template" className="flex flex-col gap-3 py-2" noValidate onSubmit={submit}>
        {make.error !== null && (!placed || unplaced) && <div ref={errorElement} tabIndex={-1}><ErrorAlert error={make.error} /></div>}
        <div className="flex"><TemplateGuideLink /></div>
        <Input label={t('templates.new.name')} {...form.field('name')} disabled={make.isPending} autoComplete="off" />
        <HomePicker value={form.values.home_id} onChange={(value) => form.set('home_id', value)} error={form.field('home_id').error} disabled={make.isPending} />
        <div className="flex flex-col gap-2">
          <RadioGroup
            label={t('templates.new.kind')}
            value={kind}
            onValueChange={(next) => {
              setKind(next);
              setTyped(null);
              setAttempt((value) => value + 1); setValidDraft(true); make.reset();
            }}
            options={purposes}
            disabled={make.isPending}
          />
        </div>
        <TemplateExamples sources={exampleSources} disabled={make.isPending || !validDraft} onChoose={(nextDriver, next) => {
          setChosen(nextDriver);
          setTyped(next);
          setAttempt((value) => value + 1); setValidDraft(true); make.reset();
        }} />
        {drivers.isPending ? (
          <SkeletonRows rows={1} />
        ) : drivers.isError ? (
          <ErrorState compact error={drivers.error} retrying={drivers.isRefetching} onRetry={() => void drivers.refetch()} />
        ) : driver === null ? null : (
          <ChoiceLocator
            label={t('templates.new.driver')}
            value={driver.name}
            onValueChange={(next) => {
              setChosen(next);
              setTyped(null);
              setAttempt((value) => value + 1); setValidDraft(true); make.reset();
            }}
            options={driverOptions}
            disabled={make.isPending}
          />
        )}
        {drivers.isSuccess && <TemplateHosts kind={kind} driver={driver?.name ?? null} hasSchema={schema !== null} />}
        {driver !== null && schema !== null && <QuickTemplateFields key={`${driver.name}:${kind}:${attempt}`} schema={schema} spec={spec} driver={driver.name} problems={problems} disabled={make.isPending} onChange={(next) => { setTyped(next); make.reset(); }} onReady={setValidDraft} />}
        <p className="text-xs text-text-muted">{t('templateSettings.finishOnPage')}</p>
      </form>
    </Dialog>
  );
}
