{{flutter_js}}
{{flutter_build_config}}

// Where the engine may fetch from, by the `[oo.assets] cdn` of the public ai6.cfg beside this
// page, which `ai6 config public` writes. The engine reads it before the app starts, so this file,
// not the app, reads that one key. "none" (the default) loads nothing from another origin:
// CanvasKit from canvaskit/ and fallback glyph fonts from assets/fonts/fallback/, which
// tool/cdn_files.dart fills. "fonts" takes fallback fonts from Google Fonts; "all" also takes the
// engine from Flutter's CDN.
const ooOrigins = {
  "none": {"canvasKitBaseUrl": "canvaskit/", "fontFallbackBaseUrl": "assets/fonts/fallback/"},
  "fonts": {"canvasKitBaseUrl": "canvaskit/"},
  "all": {}
};

// The `cdn` value of `[oo.assets]`, in the formats ai6.cfg takes: JSON after a `#!json` line, YAML
// after `#!yaml`, TOML otherwise. Anything unreadable is "none".
async function ooCdn() {
  let text;
  try {
    const response = await fetch('ai6.cfg', {cache: 'no-cache'});
    if (!response.ok) return 'none';
    text = await response.text();
  } catch (_) {
    return 'none';
  }
  const lines = text.split(/\r?\n/);
  const first = lines[0].trim();
  let value = null;
  try {
    if (first === '#!json') {
      value = JSON.parse(lines.slice(1).join('\n'))?.oo?.assets?.cdn;
    } else if (first === '#!yaml') {
      // Three levels by their indentation: oo:, then assets:, then cdn:.
      let section = false;
      let table = null;
      for (const line of lines.slice(1)) {
        if (/^\S/.test(line)) { section = /^oo:\s*(#.*)?$/.test(line); table = null; continue; }
        const indent = line.search(/\S/);
        if (!section || indent < 0) continue;
        if (table === null || indent <= table.indent) {
          table = /^\s+assets:\s*(#.*)?$/.test(line) ? {indent} : null;
          continue;
        }
        const match = line.match(/^\s+cdn:\s*["']?(\w+)["']?\s*(#.*)?$/);
        if (match) value = match[1];
      }
    } else {
      let table = '';
      for (const line of lines) {
        const header = line.match(/^\s*\[([^\]]+)\]\s*(#.*)?$/);
        if (header) table = header[1].trim();
        const match = table === 'oo.assets' && line.match(/^\s*cdn\s*=\s*"(\w+)"\s*(#.*)?$/);
        if (match) value = match[1];
      }
    }
  } catch (_) {
    value = null;
  }
  if (value in ooOrigins) return value;
  if (value != null) console.warn('ai6.cfg: [oo.assets] cdn is not none, fonts or all; using none');
  return 'none';
}

// The loader reads the CanvasKit base itself; the engine reads the rest only from
// initializeEngine, which onEntrypointLoaded must therefore pass the same config.
ooCdn().then((cdn) => _flutter.loader.load({
  config: ooOrigins[cdn],
  onEntrypointLoaded: async function (engineInitializer) {
    try {
      const engine = await engineInitializer.initializeEngine(ooOrigins[cdn]);
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
}));
