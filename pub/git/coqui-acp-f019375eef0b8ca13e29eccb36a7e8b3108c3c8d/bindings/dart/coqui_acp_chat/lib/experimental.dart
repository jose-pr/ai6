/// Experimental: what a session hub would need of a chat, which may change or go. `ChatSessionHub` adds `replay`, `applyReplay` and
/// `settleElsewhere` to a `ChatSession`; `ChatSessionShare` and `ChatSessionView` show one conversation in more than one place (a
/// window popped out of the one that holds it) over a `ChatViewLink`. The `_meta` key that names the client a message came from is
/// provisional, because the wire a hub carries them on is not specified.
library;

export 'src/link.dart';
export 'src/session.dart'
    show ChatSessionHub, chatAtMetaKey, chatClientMetaKey, chatNamesMetaKey;
export 'src/share.dart';
