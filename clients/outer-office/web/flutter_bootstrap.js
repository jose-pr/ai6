{{flutter_js}}
{{flutter_build_config}}

_flutter.loader.load({
  onEntrypointLoaded: async function (engineInitializer) {
    try {
      const engine = await engineInitializer.initializeEngine();
      await engine.runApp();
      document.querySelector('#startup')?.remove();
    } catch (_) {
      const startup = document.querySelector('#startup');
      if (startup) {
        startup.setAttribute('aria-live', 'assertive');
        startup.textContent = 'Outer Office could not start. Reload this page to try again.';
        const reload = document.createElement('button');
        reload.type = 'button';
        reload.textContent = 'Reload page';
        reload.addEventListener('click', () => window.location.reload());
        startup.append(' ', reload);
      }
    }
  },
});
