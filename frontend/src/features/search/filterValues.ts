/** Current readable names for the selected search filters, shared by the controls and their chips. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import { referencePreviewQuery } from '@/api/references';
import { selectedPersonQuery } from '@/api/trace';
import { rememberedFilterNames } from '@/features/search/filterNames';
import type { SearchParams } from '@/features/search/params';
import type { NameHit, UserRef } from '@/generated/api-types';

export type PersonFilter = 'owner' | 'created_by' | 'edited_by' | 'author';

export function useFilterValues(params: SearchParams) {
  const { t } = useTranslation();
  const [picked, setPicked] = useState<Record<string, string>>({});
  const owner = useQuery(selectedPersonQuery(params.owner ?? ''));
  const creator = useQuery(selectedPersonQuery(params.created_by ?? ''));
  const editor = useQuery(selectedPersonQuery(params.edited_by ?? ''));
  const author = useQuery(selectedPersonQuery(params.author ?? ''));
  const scope = useQuery({ ...nodeQuery(params.scope ?? ''), enabled: params.scope !== undefined }).data;
  const document = useQuery({ ...referencePreviewQuery({ kind: 'document', id: params.document_id ?? '' }), enabled: params.document_id !== undefined }).data;
  const chat = useQuery({ ...referencePreviewQuery({ kind: 'chat', id: params.chat_id ?? '' }), enabled: params.chat_id !== undefined }).data;
  const people: Record<PersonFilter, UserRef | undefined> = { owner: owner.data, created_by: creator.data, edited_by: editor.data, author: author.data };
  const errors: Record<PersonFilter, unknown> = { owner: owner.error, created_by: creator.error, edited_by: editor.error, author: author.error };
  const personName = (field: PersonFilter) => {
    const id = params[field];
    if (id === undefined) return null;
    const user = people[field];
    if (user !== undefined) return user.display_name;
    if (errors[field] !== null) return t('search.filters.unavailable');
    return picked[id] ?? t('search.filters.somebody');
  };
  const place = params.document_id !== undefined ? document?.title ?? picked[params.document_id] ?? t('search.kinds.document')
    : params.chat_id !== undefined ? chat?.title ?? picked[params.chat_id] ?? t('search.hit.chat')
      : params.scope === undefined ? null : scope?.name ?? picked[params.scope] ?? t('search.filters.aPlace');
  const remember = (hit: NameHit | null) => {
    if (hit !== null) setPicked((before) => rememberedFilterNames(before, params, hit));
  };
  return { people, errors, personName, place, remember };
}

export type FilterValues = ReturnType<typeof useFilterValues>;
