/** A skill's draft: its files, the text ones edited in the code view and the others uploaded, replaced or deleted; Publish makes the next version of it. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { FilePlus, FileUp, Send, Trash2 } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  deleteSkillDraftFile, discardSkillDraft, publishSkillDraft, putSkillDraftFile, skillDraftFileQuery, skillDraftKey, skillDraftQuery, skillKey, skillVersionsKey, startSkillDraft,
} from '@/api/skills';
import { languageOf } from '@/features/files/languages';
import type { Skill, SkillDraft } from '@/generated/api-types';
import { UploadTask } from '@/lib/uploads';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { CodeView } from '@/ui/CodeView';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { SkillRefusal } from './Refusal';

const SKILL_FILE = 'SKILL.md';

/** The draft of a skill, or the way to begin one. */
export function SkillDraftPanel({ skill }: { skill: Skill }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const draft = useQuery({ ...skillDraftQuery(skill.id), enabled: skill.draft !== null });
  const start = useMutation({
    mutationFn: () => startSkillDraft(skill.id),
    onSuccess: (made) => { cache.setQueryData(skillDraftKey(skill.id), made); void cache.invalidateQueries({ queryKey: skillKey(skill.id) }); },
  });
  if (skill.draft === null) {
    return <EmptyState icon={<FilePlus aria-hidden />} title={t('resourceDetails.skill.draft.none')}
      action={skill.latest === null ? undefined : <div className="flex flex-col items-center gap-2">
        <Button variant="primary" pending={start.isPending} onClick={() => start.mutate()}>{t('resourceDetails.skill.draft.start')}</Button>
        {start.error !== null && <SkillRefusal error={start.error} />}
      </div>} />;
  }
  if (draft.data === undefined) return draft.error === null ? <SkeletonRows rows={4} /> : <ErrorState error={draft.error} onRetry={() => void draft.refetch()} />;
  return <DraftEditor skill={skill} draft={draft.data} />;
}

function DraftEditor({ skill, draft }: { skill: Skill; draft: SkillDraft }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const [selected, setSelected] = useState(draft.files.some((file) => file.path === SKILL_FILE) ? SKILL_FILE : draft.files[0]?.path ?? '');
  const [adding, setAdding] = useState<string | null>(null);
  const [discarding, setDiscarding] = useState(false);
  const upload = useRef<HTMLInputElement>(null);
  const [uploadTo, setUploadTo] = useState<string | null>(null);
  const kept = (next: SkillDraft) => cache.setQueryData(skillDraftKey(skill.id), next);
  const file = draft.files.find((one) => one.path === selected) ?? null;
  const put = useMutation({ mutationFn: putSkillDraftFile.bind(null, skill.id), onSuccess: kept });
  const remove = useMutation({
    mutationFn: (path: string) => deleteSkillDraftFile(skill.id, path, draft.info.revision),
    onSuccess: (next) => { kept(next); setSelected(next.files[0]?.path ?? ''); },
  });
  const publish = useMutation({
    mutationFn: () => publishSkillDraft(skill.id, draft.info.revision),
    onSuccess: () => {
      cache.removeQueries({ queryKey: skillDraftKey(skill.id) });
      void cache.invalidateQueries({ queryKey: skillKey(skill.id) });
      void cache.invalidateQueries({ queryKey: skillVersionsKey(skill.id) });
    },
  });
  const discard = useMutation({
    mutationFn: () => discardSkillDraft(skill.id),
    onSuccess: () => { cache.removeQueries({ queryKey: skillDraftKey(skill.id) }); void cache.invalidateQueries({ queryKey: skillKey(skill.id) }); setDiscarding(false); },
  });
  const sendFile = useMutation({
    mutationFn: async ({ chosen, path }: { chosen: File; path: string }) => {
      const uploaded = await new UploadTask(chosen).run(new AbortController().signal);
      return putSkillDraftFile(skill.id, { path, blob: uploaded.blob, expected_revision: draft.info.revision });
    },
    onSuccess: (next, { path }) => { kept(next); setSelected(path); },
  });
  const busy = put.isPending || remove.isPending || publish.isPending || sendFile.isPending;
  const error = put.error ?? remove.error ?? publish.error ?? sendFile.error;
  return <div className="flex min-w-0 flex-col gap-3">
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
      <span className="text-sm text-text-muted">{t('resourceDetails.skill.draft.files', { count: draft.files.length })} · <ByteSize bytes={draft.files.reduce((sum, one) => sum + one.size, 0)} /></span>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" icon={<Trash2 aria-hidden />} disabled={busy} onClick={() => setDiscarding(true)}>{t('resourceDetails.skill.draft.discard')}</Button>
        <Button size="sm" variant="primary" icon={<Send aria-hidden />} pending={publish.isPending} disabled={busy} onClick={() => publish.mutate()}>{t('resourceDetails.skill.draft.publish')}</Button>
      </div>
    </div>
    {error !== null && <SkillRefusal error={error} />}
    <div className="grid min-w-0 gap-3 md:grid-cols-[minmax(12rem,16rem)_minmax(0,1fr)]">
      <div className="flex min-w-0 flex-col gap-2">
        <ul aria-label={t('resourceDetails.skill.draft.list')} className="flex flex-col rounded-lg border border-border">
          {draft.files.map((one) => <li key={one.path}>
            <button type="button" aria-current={one.path === selected ? 'true' : undefined} onClick={() => setSelected(one.path)}
              className={cx('focus-inset flex w-full min-w-0 items-baseline justify-between gap-2 px-3 py-1.5 text-left text-sm hover:bg-bg-muted', one.path === selected && 'bg-bg-muted font-medium')}>
              <span className="min-w-0 break-all font-mono">{one.path}</span>
              {one.executable && <span className="shrink-0 text-xs text-text-muted">{t('resourceDetails.skill.executableShort')}</span>}
            </button>
          </li>)}
        </ul>
        {adding === null
          ? <div className="flex flex-wrap gap-2">
            <Button size="sm" icon={<FilePlus aria-hidden />} disabled={busy} onClick={() => setAdding('')}>{t('resourceDetails.skill.draft.newFile')}</Button>
            <Button size="sm" icon={<FileUp aria-hidden />} disabled={busy} onClick={() => { setUploadTo(null); upload.current?.click(); }}>{t('resourceDetails.skill.draft.uploadFile')}</Button>
          </div>
          : <form className="flex flex-col gap-2" onSubmit={(event) => {
            event.preventDefault();
            const path = adding.trim();
            if (path === '') return;
            put.mutate({ path, text: '', expected_revision: draft.info.revision }, { onSuccess: () => { setSelected(path); setAdding(null); } });
          }}>
            <Input label={t('resourceDetails.skill.draft.path')} value={adding} spellCheck={false} autoComplete="off" disabled={busy} onChange={(event) => setAdding(event.target.value)} data-autofocus />
            <div className="flex gap-2">
              <Button size="sm" type="submit" variant="primary" disabled={adding.trim() === ''} pending={put.isPending}>{t('common.add')}</Button>
              <Button size="sm" onClick={() => setAdding(null)}>{t('common.cancel')}</Button>
            </div>
          </form>}
        <input ref={upload} type="file" hidden data-testid="skill-draft-upload" onChange={(event) => {
          const chosen = event.target.files?.[0];
          event.target.value = '';
          if (chosen !== undefined) sendFile.mutate({ chosen, path: uploadTo ?? chosen.name });
        }} />
      </div>
      {file === null ? <EmptyState compact title={t('resourceDetails.skill.draft.empty')} />
        : <DraftFile key={`${file.path}:${draft.info.revision}`} skill={skill} draft={draft} path={file.path} executable={file.executable} size={file.size} busy={busy}
          onSave={(text) => put.mutate({ path: file.path, text, expected_revision: draft.info.revision })}
          onExecutable={(executable) => put.mutate({ path: file.path, executable, expected_revision: draft.info.revision })}
          onReplace={() => { setUploadTo(file.path); upload.current?.click(); }}
          onDelete={() => remove.mutate(file.path)} />}
    </div>
    <ConfirmDialog open={discarding} onOpenChange={setDiscarding} title={t('resourceDetails.skill.draft.discardTitle')} confirmLabel={t('resourceDetails.skill.draft.discard')} danger
      pending={discard.isPending} error={discard.error} onConfirm={() => discard.mutate()}>{t('resourceDetails.skill.draft.discardHelp')}</ConfirmDialog>
  </div>;
}

function DraftFile({ skill, draft, path, executable, size, busy, onSave, onExecutable, onReplace, onDelete }: {
  skill: Skill; draft: SkillDraft; path: string; executable: boolean; size: number; busy: boolean;
  onSave: (text: string) => void; onExecutable: (executable: boolean) => void; onReplace: () => void; onDelete: () => void;
}) {
  const { t } = useTranslation();
  const content = useQuery(skillDraftFileQuery(skill.id, path, draft.info.revision));
  // The pane is keyed by its file and the draft's revision: a saved or reloaded file starts afresh.
  const [text, setText] = useState<string | null>(null);
  const language = languageOf(path, content.data?.text?.split('\n', 1)[0] ?? null);
  const dirty = text !== null && text !== content.data?.text;
  return <div className="flex min-w-0 flex-col gap-2">
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
      <span className="min-w-0 break-all font-mono text-sm">{path}</span>
      <div className="flex flex-wrap items-center gap-2">
        <Switch label={t('resourceDetails.skill.executable')} checked={executable} disabled={busy} onCheckedChange={onExecutable} />
        {content.data?.text !== null && <Button size="sm" variant="primary" disabled={!dirty || busy} onClick={() => text !== null && onSave(text)}>{t('resources.edit.save')}</Button>}
        <Button size="sm" icon={<FileUp aria-hidden />} disabled={busy} onClick={onReplace}>{t('resourceDetails.skill.draft.replace')}</Button>
        {path !== SKILL_FILE && <Button size="sm" icon={<Trash2 aria-hidden />} disabled={busy} onClick={onDelete}>{t('resourceDetails.skill.draft.delete')}</Button>}
      </div>
    </div>
    {content.data === undefined ? content.error === null ? <SkeletonRows rows={6} /> : <ErrorState error={content.error} onRetry={() => void content.refetch()} />
      : content.data.text === null ? <EmptyState compact title={t('resourceDetails.skill.binary')} description={<ByteSize bytes={size} />} />
        : <div className="h-96 min-w-0 overflow-hidden rounded-lg border border-border">
          <CodeView text={content.data.text} version={`${path}:${draft.info.revision}`} label={path} language={language?.load ?? null} languageKey={language?.id ?? null}
            onChange={setText} onSave={() => text !== null && dirty && onSave(text)} className="h-full" />
        </div>}
  </div>;
}
