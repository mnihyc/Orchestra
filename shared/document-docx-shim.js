/** What mammoth's browser build asks of the page it runs in, and the sandbox has none of: a queue for the timers its promises schedule, drained by the host, and UTF-8 reading. */
(function (global) {
  var queue = [];
  global.setTimeout = function (callback) { queue.push(callback); return queue.length; };
  global.setImmediate = global.setTimeout;
  global.clearTimeout = function () {};
  global.clearImmediate = function () {};
  /** Runs what was scheduled, and what that schedules, up to a bound; says how many ran. */
  global.__drain = function (most) {
    var count = 0;
    while (queue.length > 0 && count < most) { var next = queue.shift(); next(); count += 1; }
    return count;
  };
  if (typeof global.TextDecoder === 'undefined') {
    global.TextDecoder = function TextDecoder() {};
    global.TextDecoder.prototype.decode = function (input) {
      if (input === undefined) return '';
      var bytes = input instanceof Uint8Array ? input : (ArrayBuffer.isView(input) ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength) : new Uint8Array(input));
      var out = [];
      var at = 0;
      while (at < bytes.length) {
        var byte = bytes[at++];
        var code;
        if (byte < 0x80) code = byte;
        else if (byte >= 0xc2 && byte < 0xe0 && at < bytes.length) code = ((byte & 0x1f) << 6) | (bytes[at++] & 0x3f);
        else if (byte >= 0xe0 && byte < 0xf0 && at + 1 < bytes.length) { code = ((byte & 0x0f) << 12) | ((bytes[at] & 0x3f) << 6) | (bytes[at + 1] & 0x3f); at += 2; }
        else if (byte >= 0xf0 && byte < 0xf5 && at + 2 < bytes.length) { code = ((byte & 0x07) << 18) | ((bytes[at] & 0x3f) << 12) | ((bytes[at + 1] & 0x3f) << 6) | (bytes[at + 2] & 0x3f); at += 3; }
        else code = 0xfffd;
        if (code > 0xffff) { code -= 0x10000; out.push(String.fromCharCode(0xd800 + (code >> 10), 0xdc00 + (code & 0x3ff))); } else out.push(String.fromCharCode(code));
      }
      return out.join('');
    };
  }
})(globalThis);
