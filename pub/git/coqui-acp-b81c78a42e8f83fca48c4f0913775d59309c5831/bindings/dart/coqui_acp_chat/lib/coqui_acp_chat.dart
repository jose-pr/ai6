/// The state of an agent chat built from ACP session updates, with no Flutter and no transport.
///
/// A [ChatStore] holds [ChatSession]s; a [ChatConnection] (an [AcpChatConnection] over a `coqui_acp` client, or a channel of the
/// host's own) feeds a session its updates and takes its prompts. See the package's `AGENTS.md` for the whole API. What would
/// serve a conversation to other clients is in `experimental.dart`.
library;

export 'src/children.dart';
export 'src/connection.dart';
export 'src/content.dart';
export 'src/limits.dart';
export 'src/notifier.dart';
export 'src/options.dart';
export 'src/rendering.dart';
export 'src/session.dart' hide ChatSessionHub, chatClientMetaKey;
export 'src/store.dart';
export 'src/text.dart' show BoundedText, shortText;
export 'src/usage.dart';
