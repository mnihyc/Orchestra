/** Ready browser, helper and host entries keep credentials out of safe opening rows. */
import { useTranslation } from 'react-i18next';
import { PortCopy } from './Copy';
import type { PortEntry, PortLinkDelivery } from '@/generated/api-types';

export function PortEntries({ entries }: { entries: readonly PortEntry[] }) {
  const { t } = useTranslation();
  return <div className="flex min-w-0 flex-col gap-2">{entries.map((entry, index) => <div key={`${entry.method}:${index}`} className="min-w-0 text-xs text-text-muted"><p className="font-medium">{t(entry.method === 'browser' ? 'ports.inBrowser' : entry.method === 'helper' ? 'ports.withHelper' : 'ports.hostAndPort')} ({entry.transports.map((transport) => transport.toUpperCase()).join(', ')})</p>
    {entry.address !== null && <div className="mt-1 flex flex-wrap items-start gap-2"><code className="break-all">{entry.address}</code><PortCopy value={entry.address} label={t('ports.copyAddress')} /></div>}
    {entry.method === 'browser' && <p className="mt-1">{t(entry.needs === 'link' ? 'ports.linkRequired' : 'ports.signInEntry')}</p>}
    {entry.command !== null && <>{(entry.method === 'helper' ? entry.transports : [null]).map((transport) => { const base = entry.command ?? ''; const command = transport === null ? base : /--transport\s+\w+/.test(base) ? base.replace(/--transport\s+\w+/, `--transport ${transport}`) : `${base} --transport ${transport}`; return <div key={transport ?? 'command'}><code className="mt-1 block break-all">{command}</code><PortCopy value={command} label={transport === null ? t('ports.copyCommand') : `${t('ports.copyCommand')} (${transport.toUpperCase()})`} /></div>; })}{entry.method === 'helper' && <p className="mt-1">{t(entry.needs === 'link' ? 'ports.linkStdin' : 'ports.tokenStdin')}</p>}</>}
  </div>)}</div>;
}
/** This component belongs only to a mounted direct reply, never to query or portable state. */
export function OnceLink({ delivery, transports }: { delivery: Pick<PortLinkDelivery, 'url_once' | 'browser_url_once' | 'helper_link_once'>; transports: readonly ('tcp' | 'udp')[] }) {
  const { t } = useTranslation();
  return <div className="flex min-w-0 flex-col gap-3 rounded-lg border border-border p-3" data-testid="once-link">
    <h3 className="text-sm font-medium">{t('ports.linkCreated')}</h3>
    {delivery.url_once !== null && <div><code className="block break-all text-sm">{delivery.url_once}</code><PortCopy value={delivery.url_once} label={t('ports.copyLink')} /></div>}
    {delivery.browser_url_once !== null && <div><p className="text-sm font-medium">{t('ports.browserWebhook')}</p><code className="block break-all text-sm">{delivery.browser_url_once}</code><PortCopy value={delivery.browser_url_once} label={t('ports.copyWebhook')} /><p className="text-xs text-text-muted">{t('ports.webhookPath')}</p></div>}
    {delivery.helper_link_once !== null && <div><p className="text-sm font-medium">{t('ports.forHelper')}</p>{delivery.helper_link_once !== delivery.url_once && <><code className="block break-all text-sm">{delivery.helper_link_once}</code><PortCopy value={delivery.helper_link_once} label={t('ports.copyHelperLink')} /></>}{transports.map((transport) => { const command = `orch-connector port forward --transport ${transport} --link-stdin`; return <div key={transport} className="mt-2"><code className="block break-all text-sm">{command}</code><PortCopy value={command} label={`${t('ports.copyCommand')} (${transport.toUpperCase()})`} /></div>; })}<p className="mt-1 text-xs text-text-muted">{t('ports.linkStdin')}</p><p className="mt-1 text-xs text-text-muted">{t('ports.phoneForward')}</p></div>}
  </div>;
}
