/** Local embeddings follows the server's exact loopback transport rule without allowing external plaintext. */
import { expect, it } from 'vitest';
import { endpointFault, runtimeLoopback } from '@/features/resources/providers/Endpoints';
import { i18n } from '@/i18n';
it.each(['http://127.0.0.1:8090/v1','http://127.1.2.3:8090/v1','http://localhost:8090/v1','http://[::1]:8090/v1','http://[0:0:0:0:0:0:0:1]:8090/v1'])('allows the runtime loopback %s', (address) => {
  expect(runtimeLoopback(address)).toBe(true);
  expect(endpointFault({id:'endpoint', protocol:'openai-embeddings', baseUrl:address, options:{auth:'none'}}, false, i18n.t)).toBeNull();
});
it.each(['http://10.0.0.1:8090/v1','http://127.0.0.1.example.com/v1','http://user@127.0.0.1:8090/v1','http://2130706433:8090/v1','http://127.00.0.1:8090/v1','http://[::ffff:127.0.0.1]:8090/v1'])('keeps the HTTP policy for %s', (address) => {
  expect(runtimeLoopback(address)).toBe(false);
  expect(endpointFault({id:'endpoint', protocol:'openai-embeddings', baseUrl:address, options:{auth:'none'}}, false, i18n.t)).toBe(i18n.t('resources.endpoint.httpRefused'));
});
