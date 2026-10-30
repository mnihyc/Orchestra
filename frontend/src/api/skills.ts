/** Skills: the resource, its versions with their files, its draft, and the uploads and imports that make versions. */
import { queryOptions } from '@tanstack/react-query';
import { api, done, ok, repeatable } from '@/api/client';
import { cachedRow } from '@/api/cache';
import { queryClient } from '@/api/query';
import { resourceRows } from '@/api/listed';
import type {
  Skill, SkillCreateBody, SkillDraft, SkillDraftFilePutBody, SkillReimportReply, SkillSource, SkillUpdateBody, SkillVersion, SkillVersionInfo, SkillVersionInfoPage,
} from '@/generated/api-types';

export const skillKey = (id: string) => ['skill', id] as const;
export const skillVersionsKey = (id: string) => ['skill-versions', id] as const;
export const skillDraftKey = (id: string) => ['skill-draft', id] as const;

export const skillsQuery = queryOptions({ queryKey: ['skills'] as const, queryFn: ({ signal }) => ok(api.GET('/skills', { signal })).then(resourceRows<Skill>) });
export const skillQuery = (id: string) => queryOptions({ queryKey: skillKey(id), initialData: () => cachedRow(queryClient, 'skill', id), queryFn: ({ signal }): Promise<Skill> => ok(api.GET('/skills/{id}', { params: { path: { id } }, signal })) });
export const skillVersionsQuery = (id: string,cursor:string|null=null) => queryOptions({ queryKey: cursor===null?skillVersionsKey(id):[...skillVersionsKey(id),cursor], queryFn: ({ signal }): Promise<SkillVersionInfoPage> => ok(api.GET('/skills/{id}/versions', { params: { path: { id },query:cursor===null?{}:{cursor} }, signal })) });
export const skillVersionQuery = (id: string, version: string) => queryOptions({ queryKey: ['skill-version', id, version] as const, queryFn: ({ signal }): Promise<SkillVersion> => ok(api.GET('/skills/{id}/versions/{version_id}', { params: { path: { id, version_id: version } }, signal })) });
export const skillDraftQuery = (id: string) => queryOptions({ queryKey: skillDraftKey(id), queryFn: ({ signal }): Promise<SkillDraft> => ok(api.GET('/skills/{id}/draft', { params: { path: { id } }, signal })) });

/** What a file of a skill holds: its text where it reads as UTF-8, otherwise only its bytes. */
export interface SkillFileContent { text: string | null; bytes: number }

async function contentOf(blob: Blob): Promise<SkillFileContent> {
  const buffer = await blob.arrayBuffer();
  try {
    return { text: new TextDecoder('utf-8', { fatal: true }).decode(buffer), bytes: buffer.byteLength };
  } catch {
    return { text: null, bytes: buffer.byteLength };
  }
}

export const skillVersionFileQuery = (id: string, version: string, path: string) => queryOptions({
  queryKey: ['skill-version-file', id, version, path] as const,
  // A version never changes: what was read of it stays true.
  staleTime: Infinity,
  queryFn: async ({ signal }): Promise<SkillFileContent> => contentOf(await ok(api.GET('/skills/{id}/versions/{version_id}/file', { params: { path: { id, version_id: version }, query: { path } }, parseAs: 'blob', signal }))),
});
export const skillDraftFileQuery = (id: string, path: string, revision: number) => queryOptions({
  queryKey: ['skill-draft-file', id, path, revision] as const,
  queryFn: async ({ signal }): Promise<SkillFileContent> => contentOf(await ok(api.GET('/skills/{id}/draft/file', { params: { path: { id }, query: { path } }, parseAs: 'blob', signal }))),
});

export const createSkill = (body: SkillCreateBody, key: string): Promise<Skill> => ok(api.POST('/skills', { body, ...repeatable(key) }));
export const updateSkill = (id: string, body: SkillUpdateBody): Promise<Skill> => ok(api.PATCH('/skills/{id}', { params: { path: { id } }, body }));
export const deleteSkill = (id: string): Promise<void> => done(api.DELETE('/skills/{id}', { params: { path: { id } } }));
export const createSkillVersion = (id: string, from: SkillSource): Promise<SkillVersion> => ok(api.POST('/skills/{id}/versions', { params: { path: { id } }, body: { from } }));
export const reimportSkill = (id: string): Promise<SkillReimportReply> => ok(api.POST('/skills/{id}/reimport', { params: { path: { id } } }));
export const startSkillDraft = (id: string, fromVersion?: string): Promise<SkillDraft> => ok(api.POST('/skills/{id}/draft', { params: { path: { id } }, body: fromVersion === undefined ? {} : { from_version: fromVersion } }));
export const discardSkillDraft = (id: string): Promise<void> => done(api.DELETE('/skills/{id}/draft', { params: { path: { id } } }));
export const putSkillDraftFile = (id: string, body: SkillDraftFilePutBody): Promise<SkillDraft> => ok(api.PUT('/skills/{id}/draft/file', { params: { path: { id } }, body }));
export const deleteSkillDraftFile = (id: string, path: string, revision: number): Promise<SkillDraft> => ok(api.DELETE('/skills/{id}/draft/file', { params: { path: { id }, query: { path, expected_revision: revision } } }));
export const publishSkillDraft = (id: string, revision: number): Promise<SkillVersion> => ok(api.POST('/skills/{id}/draft/publish', { params: { path: { id } }, body: { expected_revision: revision } }));

/** The version a pinned setting names, among a skill's versions: its number, for the words that show it. */
export function versionNumber(versions: readonly SkillVersionInfo[] | undefined, id: string): number | null {
  return versions?.find((version) => version.id === id)?.number ?? null;
}
