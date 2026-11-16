/* The HTML starts these reads before its module graph occupies the connection pool. */
// eslint-disable-next-line @typescript-eslint/no-unused-expressions -- The HTML transform appends this function's invocation arguments.
(function (scope, plan) {
  if (scope.__orchStartupReads) return;
  scope.__orchStartupPlan = plan;
  var controller = new scope.AbortController();
  var stopped = false, sessionTaken = false, bootstrapTaken = false;
  var session, bootstrap;
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
    stopped = true; controller.abort(); session = null; bootstrap = null;
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
    take: function (request) {
      if (stopped) return null;
      var url = new URL(request.url, scope.location.href);
      if (url.origin !== scope.location.origin || url.search) return null;
      if (request.method === 'POST' && url.pathname === '/api/v1/auth/session' && !sessionTaken && request.headers.get('x-orch-csrf') === '1') {
        sessionTaken = true; bind(request);
        var result = session; session = null; return result;
      }
      if (request.method !== 'GET' || url.pathname !== '/api/v1/me/bootstrap' || bootstrapTaken) return null;
      bootstrapTaken = true; bind(request);
      var pending = bootstrap; bootstrap = null;
      return pending.then(function (reply) {
        if (stopped || !reply) return null;
        if (request.headers.get('authorization') !== reply.authorization) { cancel(); return null; }
        return { response: reply.response, sentAt: reply.sentAt, receivedAt: reply.receivedAt };
      });
    }
  };
  scope.__orchStartupReads = provider;
  if (scope.performance) scope.performance.mark('orch/start/html-session-asked');
  var initial = read('/api/v1/auth/session', { method: 'POST', headers: { 'x-orch-csrf': '1' } });
  session = initial;
  initial.catch(function () { return undefined; });
  bootstrap = initial.then(function (reply) {
    if (!reply.response.ok || stopped) { release(false); return null; }
    return reply.response.clone().json().then(function (data) {
      if (stopped || !data || !data.user || typeof data.user.id !== 'string' || typeof data.access_token !== 'string') { release(false); return null; }
      var authorization = 'Bearer ' + data.access_token;
      var headers = { Authorization: authorization };
      try { headers['x-orch-timezone'] = scope.Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { /* The optional timezone hint can be unavailable. */ }
      var reading = read('/api/v1/me/bootstrap', { method: 'GET', headers: headers });
      release(true);
      return reading.then(function (answer) {
        answer.authorization = authorization; return answer;
      });
    }, function (error) { release(false); throw error; });
  }, function (error) { release(false); throw error; });
  bootstrap.catch(function () { return undefined; });

  preload(plan.entry || []);
  var navigation = scope.performance && scope.performance.getEntriesByType ? scope.performance.getEntriesByType('navigation')[0] : null;
  if (navigation && /^(h2|h3)/.test(navigation.nextHopProtocol)) release(false);
})
