/** These same consumer examples are parsed by the Rust feed contract. */
import { expect, it } from 'vitest';
import { processKeys } from '@/api/processes';
import { queryFeed } from '@/realtime/queryFeeds';
import { directoryFeed } from '@/realtime/stores/environment';
import cases from '../fixtures/workspace-feeds.json';

it('uses the actual environment grammar beside a thread and on an environment page', () => {
  for (const row of cases) for (const threadId of [row.thread, null]) {
    const door = { environmentId: row.environment, threadId };
    expect(directoryFeed(door, row.path)).toBe(row.directory_feed);
    expect(queryFeed(processKeys.list(door))?.feed).toBe(row.process_feed);
  }
});
