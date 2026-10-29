{{flutter_js}}
{{flutter_build_config}}

// Where the engine may fetch from, by the `[assets] cdn` of OO.cfg beside this page. The
// engine reads it before the app starts, so this file, not the app, reads that one key.
// "none" (the default) loads nothing from another origin: CanvasKit from canvaskit/ and
// fallback glyph fonts from assets/fonts/fallback/, where a deployment may put Noto fonts.
// "fonts" takes fallback fonts from Google Fonts; "all" also takes the engine from
// Flutter's CDN.
const ooOrigins = {
  "none": {"canvasKitBaseUrl": "canvaskit/", "fontFallbackBaseUrl": "assets/fonts/fallback/"},
  "fonts": {"canvasKitBaseUrl": "canvaskit/"},
  "all": {}
};

// The `cdn` value of OO.cfg's `assets` table, in the formats OO.cfg takes: JSON after a
// `#!json` line, YAML after `#!yaml`, TOML otherwise. Anything unreadable is "none".
async function ooCdn() {
  let text;
  try {
    const response = await fetch('OO.cfg', {cache: 'no-cache'});
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
      value = JSON.parse(lines.slice(1).join('\n'))?.assets?.cdn;
    } else if (first === '#!yaml') {
      let inAssets = false;
      for (const line of lines.slice(1)) {
        if (/^\S/.test(line)) inAssets = /^assets:\s*(#.*)?$/.test(line);
        const match = inAssets && line.match(/^\s+cdn:\s*["']?(\w+)["']?\s*(#.*)?$/);
        if (match) value = match[1];
      }
    } else {
      let table = '';
      for (const line of lines) {
        const header = line.match(/^\s*\[([^\]]+)\]\s*(#.*)?$/);
        if (header) table = header[1].trim();
        const match = table === 'assets' && line.match(/^\s*cdn\s*=\s*"(\w+)"\s*(#.*)?$/);
        if (match) value = match[1];
      }
    }
  } catch (_) {
    value = null;
  }
  if (value in ooOrigins) return value;
  if (value != null) console.warn(`OO.cfg: [assets] cdn = "${value}" is not none, fonts or all; using none`);
  return 'none';
}

ooCdn().then((cdn) => _flutter.loader.load({
  config: ooOrigins[cdn],
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
}));
