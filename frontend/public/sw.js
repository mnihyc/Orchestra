// The build pins the release's public application document to this worker, with the addresses of the screens it opens
// (every screen of the application but sign-in, whose document carries the installation's policy). A returning screen
// then opens without asking the network for its document: one round trip less before its first request. Development
// uses the normal page.
const shell = null;
const screens = shell ? shell.screens.map((pattern) => new RegExp(pattern)) : [];

function readShell() {
  return fetch(shell.url, { credentials: 'omit', cache: 'force-cache', integrity: shell.integrity, redirect: 'error' });
}

self.addEventListener('install', (event) => {
  event.waitUntil((async () => {
    if (shell) {
      // A failed warmup leaves normal navigation and push available.
      try { await (await readShell()).arrayBuffer(); } catch { /* The next navigation can try again. */ }
    }
    await self.skipWaiting();
  })());
});

async function shellDocument(request) {
  try {
    const source = await readShell();
    const headers = new Headers(source.headers);
    const policy = headers.get('content-security-policy') || '';
    const original = /(?:^|;)\s*script-src\s+[^;]*?'nonce-([a-f0-9]{32})'/.exec(policy);
    if (source.ok && headers.get('content-type')?.split(';')[0] === 'application/json' && original) {
      const template = await source.json();
      if (typeof template === 'string' && template.includes('nonce="__ORCH_CSP_NONCE__"')) {
        const nonce = crypto.randomUUID().replaceAll('-', '');
        headers.set('content-security-policy', policy.replaceAll("'nonce-" + original[1] + "'", "'nonce-" + nonce + "'"));
        headers.set('content-type', 'text/html; charset=utf-8');
        headers.set('cache-control', 'no-store');
        for (const header of ['content-encoding', 'content-length', 'etag', 'last-modified']) headers.delete(header);
        return new Response(template.replaceAll('__ORCH_CSP_NONCE__', nonce), { headers });
      }
    }
  } catch { /* Normal navigation also works when an installation does not serve the pinned shell. */ }
  return fetch(request);
}

self.addEventListener('fetch', (event) => {
  if (!shell || event.request.method !== 'GET' || event.request.mode !== 'navigate') return;
  const address = new URL(event.request.url);
  if (address.origin === self.location.origin && screens.some((screen) => screen.test(address.pathname))) {
    event.respondWith(shellDocument(event.request));
  }
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

// What the server pushes is a small document: { title, body, url, tag, kind, count }. A tag that a
// newer notification repeats takes the place of the one that is still on the screen.
self.addEventListener('push', (event) => {
  let content = {};
  try {
    content = event.data ? event.data.json() : {};
  } catch {
    content = { body: event.data ? event.data.text() : '' };
  }
  const title = typeof content.title === 'string' && content.title !== '' ? content.title : 'Orch';
  event.waitUntil(
    self.registration.showNotification(title, {
      body: typeof content.body === 'string' ? content.body : '',
      tag: typeof content.tag === 'string' ? content.tag : undefined,
      icon: '/icons/orch.svg',
      data: { url: typeof content.url === 'string' ? content.url : '/inbox' },
    }),
  );
});

// Only an address of this application is opened, whatever the push said.
function addressOf(data) {
  const wanted = data && typeof data.url === 'string' ? data.url : '/inbox';
  try {
    const url = new URL(wanted, self.location.origin);
    return url.origin === self.location.origin ? url.href : new URL('/inbox', self.location.origin).href;
  } catch {
    return new URL('/inbox', self.location.origin).href;
  }
}

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const address = addressOf(event.notification.data);
  event.waitUntil(
    self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then((windows) => {
      const open = windows.find((client) => 'focus' in client);
      if (open === undefined) return self.clients.openWindow(address);
      // A window that is open is taken there, and brought to the front.
      return open.navigate(address).then(
        (moved) => (moved === null ? open : moved).focus(),
        () => open.focus(),
      );
    }),
  );
});
