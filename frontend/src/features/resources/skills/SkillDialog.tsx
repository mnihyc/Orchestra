/** A new skill, from an archive, from an environment's directory or begun empty; and a further version of one, from an archive or its directory. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { FileUp } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { environmentsQuery } from '@/api/environments';
import { hostsQuery } from '@/api/resources';
import { createSkill, createSkillVersion, skillKey, skillVersionsKey } from '@/api/skills';
import { EditDialog } from '@/features/resources/EditDialog';
import { HomePicker } from '@/features/resources/HomePicker';
import { Hint } from '@/features/help/Hint';
import type { Skill, SkillSource, SkillVersion } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { UploadTask } from '@/lib/uploads';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';
import { SkillRefusal } from './Refusal';

type Way = 'upload' | 'directory' | 'git' | 'url' | 'new';

/** The names a skill may have: lower-case letters, digits and single hyphens inside, at most 64. */
export const SKILL_NAME = /^[a-z0-9](?:[a-z0-9]|-(?!-)){0,62}(?<!-)$/u;
const DESCRIPTION_MAX = 1024;
const ARCHIVES = '.zip,.tar,.tgz,.tar.gz,.gz';

/** The fields of a source: an archive chosen, an environment and its directory, or a new skill's words. */
function useSource(way: Way) {
  const [file, setFile] = useState<File | null>(null);
  const [folder, setFolder] = useState('');
  const [environment, setEnvironment] = useState('');
  const [path, setPath] = useState('');
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [host, setHost] = useState('');
  const [address, setAddress] = useState('');
  const [reference, setReference] = useState('main');
  const task = useRef<{ file: File; task: UploadTask } | null>(null);
  const [sent, setSent] = useState<number | null>(null);
  const ready = way === 'upload' ? file !== null
    : way === 'directory' ? environment !== ''
      : way === 'git' ? host !== '' && address.trim() !== '' && reference.trim() !== ''
        : way === 'url' ? host !== '' && address.trim() !== ''
          : SKILL_NAME.test(name.trim()) && description.trim() !== '' && Array.from(description.trim()).length <= DESCRIPTION_MAX;
  /** The source as the API takes it, the archive uploaded first; a retry goes on with the same upload. */
  const source = async (signal: AbortSignal): Promise<SkillSource> => {
    if (way === 'directory') return { kind: 'environment', environment_id: environment, path: path.trim() };
    const inside = folder.trim() === '' ? {} : { folder: folder.trim() };
    if (way === 'git') return { kind: 'git', host_id: host, repository: address.trim(), reference: reference.trim(), ...inside };
    if (way === 'url') return { kind: 'url', host_id: host, url: address.trim(), ...inside };
    if (way === 'new') return { kind: 'new', name: name.trim(), description: description.trim() };
    if (file === null) throw new Error('no archive');
    if (task.current?.file !== file) task.current = { file, task: new UploadTask(file) };
    const uploaded = await task.current.task.run(signal, setSent);
    return { kind: 'upload', blob: uploaded.blob, name: file.name, ...(folder.trim() === '' ? {} : { folder: folder.trim() }) };
  };
  return { file, setFile, folder, setFolder, environment, setEnvironment, path, setPath, name, setName, description, setDescription, host, setHost, address, setAddress, reference, setReference, ready, source, sent };
}

function SourceFields({ way, fields, disabled }: { way: Way; fields: ReturnType<typeof useSource>; disabled: boolean }) {
  const { t } = useTranslation();
  const input = useRef<HTMLInputElement>(null);
  const environments = useQuery({ ...environmentsQuery(null), enabled: way === 'directory' });
  const managed = (environments.data ?? []).filter((row) => holds(row.access, 'environment.manage'));
  const hosts = useQuery({ ...hostsQuery(null), enabled: way === 'git' || way === 'url' });
  const usable = (hosts.data ?? []).filter((row) => holds(row.access, 'host.use'));
  const hostField = <Select label={t('resourceDetails.skill.form.host')} value={fields.host} disabled={disabled}
    placeholder={t(hosts.isPending ? 'common.loading' : 'resourceDetails.skill.form.chooseHost')}
    options={usable.map((row) => ({ value: row.id, label: row.name }))} onValueChange={fields.setHost} />;
  const folderField = <Input label={t('resourceDetails.skill.form.folder')} value={fields.folder} disabled={disabled} spellCheck={false} autoComplete="off" onChange={(event) => fields.setFolder(event.target.value)} />;
  if (way === 'upload') return <>
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <Button icon={<FileUp aria-hidden />} disabled={disabled} onClick={() => input.current?.click()}>{t('resourceDetails.skill.form.chooseArchive')}</Button>
      {fields.file !== null && <span className="min-w-0 break-all text-sm">{fields.file.name} · <ByteSize bytes={fields.file.size} /></span>}
      {fields.sent !== null && fields.file !== null && <span role="status" className="text-sm text-text-muted"><ByteSize bytes={fields.sent} /></span>}
    </div>
    <input ref={input} type="file" accept={ARCHIVES} hidden data-testid="skill-archive" onChange={(event) => fields.setFile(event.target.files?.[0] ?? null)} />
    <Input label={t('resourceDetails.skill.form.folder')} value={fields.folder} disabled={disabled} spellCheck={false} autoComplete="off" onChange={(event) => fields.setFolder(event.target.value)} />
  </>;
  if (way === 'git') return <>
    {hostField}
    <Input label={t('resourceDetails.skill.form.repository')} value={fields.address} disabled={disabled} spellCheck={false} autoComplete="off" inputMode="url" onChange={(event) => fields.setAddress(event.target.value)} />
    <Input label={t('resourceDetails.skill.form.reference')} value={fields.reference} disabled={disabled} spellCheck={false} autoComplete="off" onChange={(event) => fields.setReference(event.target.value)} />
    {folderField}
  </>;
  if (way === 'url') return <>
    {hostField}
    <Input label={t('resourceDetails.skill.form.url')} value={fields.address} disabled={disabled} spellCheck={false} autoComplete="off" inputMode="url" onChange={(event) => fields.setAddress(event.target.value)} />
    {folderField}
  </>;
  if (way === 'directory') return <>
    <Select label={t('resourceDetails.skill.form.environment')} value={fields.environment} disabled={disabled}
      placeholder={t(environments.isPending ? 'common.loading' : 'resourceDetails.skill.form.chooseEnvironment')}
      options={managed.map((row) => ({ value: row.id, label: row.name }))} onValueChange={fields.setEnvironment} />
    <Input label={t('resourceDetails.skill.form.path')} value={fields.path} disabled={disabled} spellCheck={false} autoComplete="off" onChange={(event) => fields.setPath(event.target.value)} />
  </>;
  return <>
    <Input label={t('resources.edit.name')} value={fields.name} maxLength={64} disabled={disabled} spellCheck={false} autoComplete="off" onChange={(event) => fields.setName(event.target.value)}
      error={fields.name !== '' && !SKILL_NAME.test(fields.name.trim()) ? t('resourceDetails.skill.form.nameInvalid') : undefined} data-autofocus />
    <Textarea label={t('resources.listDescription')} value={fields.description} rows={3} maxLength={DESCRIPTION_MAX} disabled={disabled} onChange={(event) => fields.setDescription(event.target.value)} />
  </>;
}

export function AddSkill({ onClose, onCreated }: { onClose: () => void; onCreated?: (skill: Skill) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const actKey = useActKey();
  const [way, setWay] = useState<Way>('upload');
  const [home, setHome] = useState('');
  const fields = useSource(way);
  const abort = useRef(new AbortController());
  const save = useMutation({
    mutationFn: async () => {
      const body = { home_id: home, from: await fields.source(abort.current.signal) };
      return createSkill(body, actKey(body));
    },
    onSuccess: (skill) => { cache.setQueryData(skillKey(skill.id), skill); onCreated?.(skill); onClose(); },
  });
  return <EditDialog title={t('resourceDetails.skill.add')} submitLabel={t('common.add')} changed valid={home !== '' && fields.ready} pending={save.isPending} error={null}
    onSave={() => save.mutate()} onClose={() => { abort.current.abort(); onClose(); }}>
    <SegmentedControl<Way> label={t('resourceDetails.skill.form.from')} value={way} onValueChange={(next) => { setWay(next); save.reset(); }} startsKeyboard segments={[
      { value: 'upload', label: t('resourceDetails.skill.form.upload') },
      { value: 'git', label: t('resourceDetails.skill.form.git') },
      { value: 'url', label: t('resourceDetails.skill.form.address') },
      { value: 'directory', label: t('resourceDetails.skill.form.directory') },
      { value: 'new', label: t('resourceDetails.skill.form.new') },
    ]} />
    <Hint id="skills" popover className="self-start" />
    <SourceFields way={way} fields={fields} disabled={save.isPending} />
    <HomePicker value={home} onChange={setHome} disabled={save.isPending} compact />
    {save.error !== null && <SkillRefusal error={save.error} />}
  </EditDialog>;
}

export function AddVersion({ skill, onClose, onMade }: { skill: Skill; onClose: () => void; onMade?: (version: SkillVersion) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const [way, setWay] = useState<Exclude<Way, 'new'>>('upload');
  const fields = useSource(way);
  const abort = useRef(new AbortController());
  const save = useMutation({
    mutationFn: async () => createSkillVersion(skill.id, await fields.source(abort.current.signal)),
    onSuccess: (version) => {
      void cache.invalidateQueries({ queryKey: skillKey(skill.id) });
      void cache.invalidateQueries({ queryKey: skillVersionsKey(skill.id) });
      onMade?.(version);
      onClose();
    },
  });
  return <EditDialog title={t('resourceDetails.skill.addVersion', { name: skill.name })} submitLabel={t('common.add')} changed valid={fields.ready} pending={save.isPending} error={null}
    onSave={() => save.mutate()} onClose={() => { abort.current.abort(); onClose(); }}>
    <SegmentedControl<Exclude<Way, 'new'>> label={t('resourceDetails.skill.form.from')} value={way} onValueChange={(next) => { setWay(next); save.reset(); }} startsKeyboard segments={[
      { value: 'upload', label: t('resourceDetails.skill.form.upload') },
      { value: 'git', label: t('resourceDetails.skill.form.git') },
      { value: 'url', label: t('resourceDetails.skill.form.address') },
      { value: 'directory', label: t('resourceDetails.skill.form.directory') },
    ]} />
    <Hint id="skills" popover className="self-start" />
    <SourceFields way={way} fields={fields} disabled={save.isPending} />
    {save.error !== null && <SkillRefusal error={save.error} />}
  </EditDialog>;
}
