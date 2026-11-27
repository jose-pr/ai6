/// What a chat keeps of what an agent sends. An agent can send any amount, so every field here bounds one thing, and each takes any
/// value from 0 up. A text limit counts UTF-16 code units, as Dart's `String.length` does.
final class ChatLimits {
  const ChatLimits({
    this.entries = 5000,
    this.messageText = 2 * 1024 * 1024,
    this.pendingPermissions = 64,
    this.pendingQuestions = 16,
    this.settledRequests = 32,
    this.queued = 20,
    this.attachments = 8,
    this.clients = 16,
    this.turns = 1000,
    this.toolText = 256 * 1024,
    this.terminalOutput = 256 * 1024,
    this.planEntries = 200,
    this.commands = 500,
    this.configOptions = 32,
    this.choices = 200,
    this.toolBlocks = 64,
    this.locations = 64,
    this.shortText = 512,
    this.terminals = 256,
    this.describedInput = 4000,
    this.hostNotices = 4,
    this.listShapes = 8,
    this.shapeDepth = 8,
    this.titleLength = 60,
    this.usageBuckets = 64,
    this.handOverSeconds = 60,
    this.children = 256,
    this.childUpdates = 1000,
  }) : assert(
         entries >= 0 &&
             messageText >= 0 &&
             pendingPermissions >= 0 &&
             pendingQuestions >= 0 &&
             settledRequests >= 0 &&
             queued >= 0 &&
             attachments >= 0 &&
             clients >= 0 &&
             turns >= 0 &&
             toolText >= 0 &&
             terminalOutput >= 0 &&
             planEntries >= 0 &&
             commands >= 0 &&
             configOptions >= 0 &&
             choices >= 0 &&
             toolBlocks >= 0 &&
             locations >= 0 &&
             shortText >= 0 &&
             terminals >= 0 &&
             describedInput >= 0 &&
             hostNotices >= 0 &&
             listShapes >= 0 &&
             shapeDepth >= 0 &&
             titleLength >= 0 &&
             usageBuckets >= 0 &&
             handOverSeconds >= 0 &&
             children >= 0 &&
             childUpdates >= 0,
       );

  /// The most entries (messages, tool lines, compactions) a conversation keeps; the oldest go first. Default 5000.
  final int entries;

  /// The longest one message keeps. Default 2 MiB.
  final int messageText;

  /// The most permission requests one conversation holds waiting for the person; the rest are answered cancelled. Default 64.
  final int pendingPermissions;

  /// The most questions one conversation holds waiting for the person; the rest are answered cancelled. Default 16.
  final int pendingQuestions;

  /// The most answered requests a conversation remembers, which a client attaching late reads to see who answered. Default 32.
  final int settledRequests;

  /// The most messages one conversation holds waiting for its turn to end. Default 20.
  final int queued;

  /// The most attachments one queued message holds when messages are joined into one. Default 8.
  final int attachments;

  /// The most clients attached to one conversation at once. Default 16.
  final int clients;

  /// The most turns a conversation remembers the length of. Default 1000.
  final int turns;

  /// The longest text of a tool's content, a diff side or a compaction summary. Default 256 KiB.
  final int toolText;

  /// The most output one command keeps, from its end, which is where a failure says why. Default 256 KiB.
  final int terminalOutput;

  /// The most entries a plan keeps. Default 200.
  final int planEntries;

  /// The most slash commands kept. Default 500.
  final int commands;

  /// The most options (model, mode, effort, ...) kept. Default 32.
  final int configOptions;

  /// The most choices one option keeps. Default 200.
  final int choices;

  /// The most content blocks one tool call keeps, and the most names of what went with a message. Default 64.
  final int toolBlocks;

  /// The most places and changed files one tool call keeps. Default 64.
  final int locations;

  /// The longest title, name or label kept; a longer one is cut with `…`. Default 512.
  final int shortText;

  /// The most commands whose output is kept. Default 256.
  final int terminals;

  /// The most of a tool's input shown to the person approving it. Default 4000.
  final int describedInput;

  /// The most notices a host's answer to `session/new` may add to a conversation. Default 4.
  final int hostNotices;

  /// The most distinct shapes a list keeps when an update's shape is recorded. Default 8.
  final int listShapes;

  /// How deep an update's shape is recorded before it ends in `…`. Default 8.
  final int shapeDepth;

  /// The longest title [titleFrom] makes of a first message, in characters. Default 60.
  final int titleLength;

  /// The most buckets, models and sources a usage report keeps of each. Default 64.
  final int usageBuckets;

  /// How long a v2 agent has to acknowledge a prompt before the message is taken as not handed over and `send` fails. 0 waits
  /// without bound. Default 60.
  final int handOverSeconds;

  /// The most child agents (subagents) one conversation keeps; the oldest ended one goes first. Default 256.
  final int children;

  /// The most of its own updates one child agent keeps, the oldest dropped. Default 1000.
  final int childUpdates;
}
