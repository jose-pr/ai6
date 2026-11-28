/// A person's agents from a web page, over `document.acp`, the page contract in `docs/browser.md`. A browser provider holds the
/// connection and proves the agent. Outside a browser there is never a provider.
library;

export 'src/page_api.dart';
export 'src/page_stub.dart' if (dart.library.js_interop) 'src/page_web.dart';
