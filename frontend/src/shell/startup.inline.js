/* The HTML starts these reads before its module graph occupies the connection pool. */
// eslint-disable-next-line @typescript-eslint/no-unused-expressions -- The HTML transform appends this function's invocation arguments.
(function (scope, plan) {
  if (scope.__orchStartupReads) return;
  scope.__orchStartupPlan = plan;
  var controller = new scope.AbortController();
  var stopped = false, sessionTaken = false;
  var session;
  var address = !scope.location.search && /^\/thread\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\/?$/i.exec(scope.location.pathname);
  var openingPath = address ? '/api/v1/threads/' + address[1] + '/opening' : null;
  var pendingReads = new Map();
  var allowShell;
  var content = new Promise(function (resolve) { allowShell = resolve; });
  if (!address) allowShell();
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
  function cancel() {
    stopped = true; controller.abort(); session = null;
    pendingReads.clear(); provider.contentShown();
    release(false);
    if (scope.__orchStartupReads === provider) delete scope.__orchStartupReads;
  }
  function bind(request) {
    if (request.signal.aborted) cancel();
    else request.signal.addEventListener('abort', cancel, { once: true });
  }
  var provider = {
    scriptsReady: scriptsReady,
    application: plan.application,
    painted: function () { painted = true; if (dispatchReady) release(frameNeeded); },
    cancel: cancel,
    afterThreadContent: address ? content : undefined,
    contentShown: function () { provider.afterThreadContent = undefined; allowShell(); },
    take: function (request) {
      if (stopped) return null;
      var url = new URL(request.url, scope.location.href);
      if (url.origin !== scope.location.origin || url.search) return null;
      if (request.method === 'POST' && url.pathname === '/api/v1/auth/session' && !sessionTaken && request.headers.get('x-orch-csrf') === '1') {
        sessionTaken = true; bind(request);
        var result = session; session = null; return result;
      }
      var pending = pendingReads.get(url.pathname);
      if (request.method !== 'GET' || !pending || pending.taken) return null;
      pending.taken = true;
      var abort = function () { pending.controller.abort(); };
      if (request.signal.aborted) abort();
      else request.signal.addEventListener('abort', abort, { once: true });
      return pending.answer.then(function (reply) {
        if (stopped || pending.controller.signal.aborted || !reply || request.headers.get('authorization') !== reply.authorization) { abort(); return null; }
        return { response: reply.response, sentAt: reply.sentAt, receivedAt: reply.receivedAt };
      }).finally(function () { pendingReads.delete(url.pathname); });
    }
  };
  scope.__orchStartupReads = provider;
  if (scope.performance) scope.performance.mark('orch/start/html-session-asked');
  function project(receipt, body, problem, retry) {
    var headers = new Headers();
    for (var name of ['date', 'x-request-id', 'cache-control']) {
      var value = receipt.response.headers.get(name);
      if (value) headers.set(name, value);
    }
    if (problem) {
      headers.set('content-type', 'application/problem+json');
      if (body.request_id) headers.set('x-request-id', body.request_id);
      if (typeof retry === 'number') headers.set('retry-after', String(retry));
    }
    return { response: Response.json(body, { status: problem ? body.status : 200, headers: headers }), sentAt: receipt.sentAt, receivedAt: receipt.receivedAt };
  }
  var post = { method: 'POST', headers: { 'x-orch-csrf': '1' } };
  var initial = read(address ? '/api/v1/auth/session/thread/' + address[1] : '/api/v1/auth/session', post);
  var compound = address ? initial.then(function (receipt) {
    if (!receipt.response.ok) { provider.contentShown(); return { receipt: receipt, body: null }; }
    return receipt.response.json().then(function (body) {
      if (!body || body.opening.status !== 'ready') provider.contentShown();
      return { receipt: receipt, body: body };
    });
  }) : null;
  session = compound ? compound.then(function (part) {
    return part.receipt.response.ok ? project(part.receipt, part.body && part.body.session) : part.receipt;
  }) : initial;
  var authenticated = session.then(function (reply) {
    if (!reply.response.ok || stopped) return null;
    return reply.response.clone().json().then(function (data) {
      if (!data || !data.user || typeof data.user.id !== 'string' || typeof data.access_token !== 'string') return null;
      var headers = { Authorization: 'Bearer ' + data.access_token };
      try { headers['x-orch-timezone'] = scope.Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { /* The optional timezone hint can be unavailable. */ }
      return headers;
    });
  });
  function keep(path, own, answer) {
    controller.signal.addEventListener('abort', function () { own.abort(); }, { once: true });
    pendingReads.set(path, { controller: own, answer: answer, taken: false });
    answer.catch(function () { return undefined; });
  }
  if (compound) {
    var openingController = new scope.AbortController();
    keep(openingPath, openingController, compound.then(function (part) {
      if (!part.body || stopped || openingController.signal.aborted) return null;
      var opening = part.body.opening;
      var reply = opening.status === 'ready' ? project(part.receipt, opening.data) : project(part.receipt, opening.problem, true, opening.retry_after);
      reply.authorization = 'Bearer ' + part.body.session.access_token;
      return reply;
    }));
    // The dispatched address already identifies the public feature code while its data is in flight.
    release(true);
  }
  var shellController = new scope.AbortController();
  keep('/api/v1/me/bootstrap', shellController, authenticated.then(function (headers) {
    if (!headers || stopped) return null;
    return content.then(function () {
      if (stopped || shellController.signal.aborted) return null;
      return read('/api/v1/me/bootstrap', { method: 'GET', headers: headers, signal: shellController.signal }).then(function (reply) {
        reply.authorization = headers.Authorization; return reply;
      });
    });
  }));
  authenticated.then(function (headers) { release(!!headers && !stopped); }, function () { provider.contentShown(); release(false); });
  preload(plan.entry || []);
  var navigation = scope.performance && scope.performance.getEntriesByType ? scope.performance.getEntriesByType('navigation')[0] : null;
  if (navigation && /^(h2|h3)/.test(navigation.nextHopProtocol)) release(false);
})
