/** A recorded text block: bounded inline text and the immutable whole behind its ordinary blob read. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { modelInputText } from '@/api/modelInputs';
import type { ModelInput, ModelTextBlock } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { copyText } from '@/ui/clipboard';
import { givenBlockWord } from './words';

export function GivenBlock({ record, block }: { record: ModelInput; block: ModelTextBlock }) {
  const { t } = useTranslation();
  const [whole, setWhole] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const name = givenBlockWord(t, block.key);
  const query = useQuery({ queryKey: ['model-input-block', record.id, block.key, block.text.blob], queryFn: ({ signal }) => modelInputText(record, block, signal), enabled: whole, retry: false });
  const copy = async () => { try { await copyText(await modelInputText(record, block)); } catch (failure) { setError(failure); } };
  return <li className="rounded-md border border-border">
    <div className="flex items-center justify-between gap-2 border-b border-border px-2 py-1 text-xs text-text-muted"><span>{name}</span><Button size="sm" variant="ghost" onClick={() => void copy()}>{t('threadViews.given.copyBlock', { block: name })}</Button></div>
    <pre className="whitespace-pre-wrap break-words p-2 font-mono text-xs">{query.data ?? block.text.inline}</pre>
    {block.text.blob !== null && !whole && <Button size="sm" variant="ghost" onClick={() => setWhole(true)}>{t('threadViews.given.readFull')}</Button>}
    {query.error !== null && <ErrorAlert error={query.error} />}{error !== null && <ErrorAlert error={error} />}
  </li>;
}
