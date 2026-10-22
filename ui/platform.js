(function (root) {
  'use strict';

  /** WebView2 and WKWebView expose the host OS without an extra startup IPC. */
  function detectPlatform(navigator = {}) {
    const value = `${navigator.userAgentData?.platform || ''} ${navigator.platform || ''} ${navigator.userAgent || ''}`;
    if (/windows|win32|win64/i.test(value)) return 'windows';
    if (/macintosh|macintel|macppc|macos|mac os/i.test(value)) return 'macos';
    if (/linux/i.test(value)) return 'linux';
    return 'unknown';
  }

  /** Keep user-visible shortcuts and their modifier policy in one place. */
  function forPlatform(current) {
    const isMac = current === 'macos';
    return {
      current,
      isMac,
      isWindows: current === 'windows',
      revealLabel: isMac ? '在 Finder 中显示' : current === 'windows' ? '在文件资源管理器中显示' : '在文件管理器中显示',
      shortcut(key, { shift = false } = {}) {
        return isMac ? `${shift ? '⇧' : ''}⌘${key}` : `Ctrl+${shift ? 'Shift+' : ''}${key}`;
      },
      isSendShortcut(event) {
        return event.key === 'Enter' && !event.isComposing && event.keyCode !== 229 && !event.repeat
          && !event.altKey && !event.shiftKey
          && (isMac ? !!event.metaKey && !event.ctrlKey : !!event.ctrlKey && !event.metaKey);
      },
    };
  }

  const api = { detectPlatform, forPlatform, ...forPlatform(detectPlatform(root.navigator)) };
  if (typeof module === 'object' && module.exports) module.exports = api;
  else {
    root.MailBeaconPlatform = api;
    root.document.documentElement.dataset.platform = api.current;
  }
})(typeof window !== 'undefined' ? window : globalThis);
