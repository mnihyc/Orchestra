/* The HTML starts these reads before its module graph occupies the connection pool. */
// eslint-disable-next-line @typescript-eslint/no-unused-expressions -- The HTML transform appends this function's invocation arguments.
(function (scope, plan) {
  if (scope.__orchStartupReads) return;
  scope.__orchStartupPlan = plan;
  var controller = new scope.AbortController();
  var stopped = false, sessionTaken = false, bootstrapTaken = false;
  var session, bootstrap, storage;
  var storagePath = '/api/v1/admin/storage';
  var isStoragePage = /^\/admin\/storage\/?$/;
  var storageController = isStoragePage.test(scope.location.pathname) ? new scope.AbortController() : null;
  var released = false, painted = false, dispatchReady = false, frameNeeded = false, releaseScripts;
  var scriptsReady = new Promise(function (resolve) { releaseScripts = resolve; });
  var files = [];
  for (var page of plan.pages) {
    if (new RegExp(page.pattern).test(scope.location.pathname)) { files.push.apply(files, page.files); break; }
  }
  var held = new Set();
  for (var link of scope.document.querySelectorAll('link[rel="modulepreload"]')) held.add(link.getAttribute('href'));
  function preload(files) {
    for (var file of files) {
      var href = '/' + file;
      if (held.has(href)) continue;
      held.add(href);
      var preload = scope.document.createElement('link');
      preload.rel = 'modulepreload'; preload.href = href; preload.crossOrigin = 'anonymous'; preload.fetchPriority = 'low';
      scope.document.head.appendChild(preload);
    }
  }
  function release(frame) {
    dispatchReady = true; frameNeeded = frameNeeded || frame;
    if (!painted) return;
    if (frameNeeded) preload(plan.common);
    if (released) return;
    released = true; preload(files); releaseScripts();
  }
  function read(path, options) {
    var sentAt = Date.now();
    return scope.fetch(path, Object.assign({ credentials: 'same-origin', signal: controller.signal, priority: 'high' }, options))
      .then(function (response) { return { response: response, sentAt: sentAt, receivedAt: Date.now() }; });
  }
  function leavePage() {
    if (storageController) storageController.abort();
    storage = null;
  }
  function cancel() {
    leavePage();
    stopped = true; controller.abort(); session = null; bootstrap = null;
    release(false);
    if (scope.__orchStartupReads === provider) delete scope.__orchStartupReads;
  }
  function bind(request) {
    if (request.signal.aborted) cancel();
    else request.signal.addEventListener('abort', cancel, { once: true });
  }
  function authorizedReply(pending, request, signal) {
    return pending.then(function (reply) {
      if (signal.aborted || !reply) return null;
      if (request.headers.get('authorization') !== reply.authorization) { cancel(); return null; }
      return { response: reply.response, sentAt: reply.sentAt, receivedAt: reply.receivedAt };
    });
  }
  function authorizedRead(path, headers, signal) {
    return read(path, { method: 'GET', headers: headers, signal: signal }).then(function (answer) {
      answer.authorization = headers.Authorization;
      return answer;
    });
  }
  var provider = {
    visit: function (path) { if (!isStoragePage.test(path)) leavePage(); },
    scriptsReady: scriptsReady,
    application: plan.application,
    painted: function () { painted = true; if (dispatchReady) release(frameNeeded); },
    cancel: cancel,
    take: function (request) {
      if (stopped) return null;
      var url = new URL(request.url, scope.location.href);
      if (url.origin !== scope.location.origin || url.search) return null;
      if (request.method === 'POST' && url.pathname === '/api/v1/auth/session' && !sessionTaken && request.headers.get('x-orch-csrf') === '1') {
        sessionTaken = true; bind(request);
        var result = session; session = null; return result;
      }
      if (request.method !== 'GET') return null;
      if (url.pathname === storagePath && storage) {
        var opening = storage; storage = null;
        if (request.signal.aborted) leavePage();
        else request.signal.addEventListener('abort', leavePage, { once: true });
        return authorizedReply(opening, request, storageController.signal);
      }
      if (url.pathname !== '/api/v1/me/bootstrap' || bootstrapTaken) return null;
      bootstrapTaken = true; bind(request);
      var pending = bootstrap; bootstrap = null;
      return authorizedReply(pending, request, controller.signal);
    }
  };
  scope.__orchStartupReads = provider;
  if (scope.performance) scope.performance.mark('orch/start/html-session-asked');
  var initial = read('/api/v1/auth/session', { method: 'POST', headers: { 'x-orch-csrf': '1' } });
  session = initial;
  initial.catch(function () { return undefined; });
  var grant = initial.then(function (reply) {
    return reply.response.ok && !stopped ? reply.response.clone().json() : null;
  }).then(function (data) {
    return !stopped && data && data.user && typeof data.user.id === 'string' && typeof data.access_token === 'string' ? data : null;
  });
  bootstrap = grant.then(function (data) {
    if (!data) { release(false); return null; }
    var headers = { Authorization: 'Bearer ' + data.access_token };
    try { headers['x-orch-timezone'] = scope.Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { /* The optional timezone hint can be unavailable. */ }
    var reading = authorizedRead('/api/v1/me/bootstrap', headers, controller.signal);
    release(true);
    return reading;
  }, function (error) { release(false); throw error; });
  bootstrap.catch(function () { return undefined; });
  if (storageController) {
    storage = grant.then(function (data) {
      if (!data || storageController.signal.aborted || data.user.is_system_admin !== true) return null;
      // Storage checks this session's current admin mode on the server.
      return authorizedRead(storagePath, { Authorization: 'Bearer ' + data.access_token, 'x-orch-view': scope.location.pathname }, storageController.signal);
    });
    storage.catch(function () { return undefined; });
  }

  preload(plan.entry || []);
  var navigation = scope.performance && scope.performance.getEntriesByType ? scope.performance.getEntriesByType('navigation')[0] : null;
  if (navigation && /^(h2|h3)/.test(navigation.nextHopProtocol)) release(false);
})
