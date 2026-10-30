// The service worker of the application: Web Push only. No offline application caching is planned.
//
// It does two things and nothing else: it shows what the server pushes, and it opens what is
// pressed. It caches nothing and answers no request, so the application is never served from here.

self.addEventListener('install', () => {
  // A new worker takes over at once: there is nothing in flight that an older one holds.
  self.skipWaiting();
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
