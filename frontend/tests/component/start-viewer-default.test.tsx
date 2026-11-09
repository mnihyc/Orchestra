/** The environment-viewer choice exposes the same resolved value and provenance at either entrance. */
import { fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { GeneralAccessControl } from '@/features/access/GeneralAccess';
import { useViewerDefault } from '@/features/start/viewerDefault';
import type { StartOptions } from '@/api/start';
import { i18n, setLocale } from '@/i18n';
import { CATALOGUE, TOPIC } from '../unit/general-access-fixtures';
import { options } from '../unit/start-fixtures';
import { show } from './support';

function Control({ offered }: { offered: StartOptions }) {
  const viewers = useViewerDefault('topic',offered);
  return viewers.value===undefined ? null : <GeneralAccessControl catalogue={CATALOGUE} kind="thread" place="Topic" value="defaults" onChange={()=>undefined}
    environment={{checked:viewers.value,onChange:viewers.onChange,tooltip:viewers.tooltip}} />;
}

afterEach(async()=>{await setLocale('en');});
for(const locale of ['en','zh-CN'] as const) it(`names the setting and its actual supplying place on focus (${locale})`,async()=>{
  await setLocale(locale);
  show(<Control offered={options({viewer_default:{value:true,source:{scope:'topic',node:TOPIC}}})} />);
  const checkbox=screen.getByRole('checkbox',{name:i18n.t('access.general.letIn')});
  expect(checkbox).toBeChecked();
  fireEvent.focus(checkbox);
  const tooltip=await screen.findByRole('tooltip');
  expect(tooltip).toHaveTextContent('threads.let_viewers_in');
  expect(tooltip).toHaveTextContent(TOPIC.name!);
});
