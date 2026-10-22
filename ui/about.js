(function (root) {
  'use strict';

  function createAboutController({ document, invoke, listen }) {
    const version = document.getElementById('about-version');
    const error = document.getElementById('about-error');
    function showError(message) {
      error.textContent = message;
      error.hidden = !message;
    }
    return {
      async connectMenuErrors() {
        try {
          return await listen('native-menu-error', event => {
            if (typeof event.payload === 'string') showError(event.payload);
          });
        } catch (cause) {
          showError(`无法接收菜单提示：${cause?.message || cause}`);
          return () => {};
        }
      },
      async load() {
        try {
          const info = await invoke('about_info');
          if (!info || typeof info.version !== 'string' || !info.version) throw new Error('未收到版本信息');
          version.textContent = `版本 ${info.version}`;
        } catch (cause) {
          version.textContent = '版本暂不可用';
          showError(`无法读取版本：${cause?.message || cause}`);
        }
      },
      async openLink(kind) {
        if (!['project', 'issues'].includes(kind)) return;
        showError('');
        try {
          await invoke('open_project_link', { kind });
        } catch (cause) {
          showError(`无法打开链接：${cause?.message || cause}`);
        }
      },
      showError,
    };
  }

  if (typeof module === 'object' && module.exports) {
    module.exports = { createAboutController };
  } else {
    const controller = createAboutController({
      document: root.document,
      invoke: (command, args) => root.__TAURI__.core.invoke(command, args),
      listen: (event, callback) => root.__TAURI__.window.getCurrentWindow().listen(event, callback),
    });
    controller.connectMenuErrors().then(stop => root.addEventListener('pagehide', stop, { once: true }));
    root.document.querySelectorAll('[data-project-link]').forEach(button => {
      button.addEventListener('click', () => controller.openLink(button.dataset.projectLink));
    });
    root.document.addEventListener('keydown', event => {
      if (event.key !== 'Escape' || event.isComposing) return;
      event.preventDefault();
      root.__TAURI__.window.getCurrentWindow().close()
        .catch(cause => controller.showError(`无法关闭窗口：${cause?.message || cause}`));
    });
    controller.load();
  }
})(typeof window === 'undefined' ? globalThis : window);
