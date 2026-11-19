/// Flutter widgets for an agent chat, over `coqui_acp_chat`: a transcript with a renderer registry, the cards (permission, question,
/// queue), a composer, the changes pane and the diff, and [ChatLook], the one theme extension they draw with.
///
/// See the package's `AGENTS.md` for the whole API.
library;

export 'src/cards.dart' show BypassBanner, PermissionCard, QuestionCard, QueuedLine, QueuePanel, UsageFooter, maxQuestionText, shortLine;
export 'src/changes.dart' show ChangeLine, ChangesPane, FileChange, changesOf;
export 'src/composer.dart';
export 'src/content.dart' show AgentMarkdown, CompactionView, CopyButton, MessageBox, Thought, TranscriptEntry, conversationText, plainMarkdown;
export 'src/diff.dart' show DiffKind, DiffLine, diffLines, patchLines, withContext;
export 'src/entry.dart';
export 'src/fold.dart' show BurstSummary, foldTranscript, spanLabel, summarizeBurst;
export 'src/folded.dart' show FoldLine, PlanChecklist, PlanPanel, SummaryText, ToolBurst, TranscriptItemView, TurnWork;
export 'src/working.dart' show ActivityMark, WorkingRow, workingHeadline;
export 'src/look.dart';
export 'src/renderers.dart';
export 'src/secret_cards.dart' show SecretCardEnvironment, SecretRequestCard, SecretRequestEntry, SecretShowCard, SecretShowEntry, secretRenderers;
export 'src/tools.dart' show CodeBox, DiffView, TerminalBlock, ToolBlockView, ToolCard, ToolEntry, toolIcon;
export 'src/transcript.dart' show ChatTranscript, ChatTranscriptState, SelectableTranscript, SelectableTranscriptState;
export 'src/usage_text.dart';
