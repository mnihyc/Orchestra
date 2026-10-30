/** Adjacent versions are read on demand; changing the viewed history never starts work. */
import { useMutation } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { adjacentPromptVersion } from '@/api/continuations';
import { selectThreadContinuation } from '@/api/threadRead';
import type { ThreadPromptSummary } from '@/generated/api-types';
import { threadViewOf } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadChanged } from '@/realtime/threadScope';
import { ErrorAlert } from '@/ui/ErrorState';
import { PromptVersions } from './PromptVersions';
import type { SelectPromptVersion } from './context';

export type { SelectPromptVersion } from './context';

export function PromptNavigation({ threadId, prompt, onSelect }: {
  threadId: string; prompt: ThreadPromptSummary; onSelect?: SelectPromptVersion | undefined;
}) {
  const read = useThreadReadRef(threadId);
  const strip = useRef<HTMLSpanElement>(null);
  const focused = useRef(false);
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const browse = useMutation({
    mutationFn: async (direction: 'previous' | 'next') => {
      const projection = threadViewOf(read).read?.projection;
      if (projection === null || projection === undefined) throw threadReadChanged();
      const page = await adjacentPromptVersion(read, prompt, projection, direction);
      const version = page.rows[0];
      if (version === undefined) throw new Error('The adjacent message version is no longer available');
      return version;
    },
    onSuccess: (version) => {
      if (!mounted.current) return;
      if (onSelect !== undefined) onSelect(version.continuation_id, { inputId: version.input_id,
        itemId: version.item_id ?? null, groupId: version.group_id, focus: focused.current
          && (document.activeElement === document.body || strip.current?.contains(document.activeElement) === true) });
      else selectThreadContinuation(read, version.continuation_id);
    },
  });
  const browseVersion = (direction: 'previous' | 'next') => {
    focused.current = strip.current?.contains(document.activeElement) === true;
    browse.mutate(direction);
  };
  return <>
    <PromptVersions ref={strip} groupId={prompt.group_id} current={prompt.ordinal} total={prompt.total} pending={browse.isPending}
      onPrevious={() => browseVersion('previous')} onNext={() => browseVersion('next')} />
    {browse.error !== null && <ErrorAlert error={browse.error} />}
  </>;
}
