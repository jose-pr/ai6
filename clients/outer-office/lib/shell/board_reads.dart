import '../settings/oo_config.dart';
import 'oo_shell.dart' show OoPage;

/// One read the board repeats.
enum BoardRead {
  board,
  missions,
  stations,
  profile,
  heldBriefs,
  proposals,
  greenInk,
}

/// What a page shows from the board and how current it must be: [live] at `[refresh] sessions`,
/// [background] at `[refresh] slow`. A read in neither is not repeated for that page.
///
/// Pages with their own reads (Green Ink, Briefings, Agents, Roster, Pools, Timeline, Audit,
/// Access) show nothing live from the board.
({Set<BoardRead> live, Set<BoardRead> background}) readsOf(OoPage page) {
  // The bell and the search are on every page: they name Sessions, Stations, held briefs, green
  // ink and proposals, so these stay read, slowly.
  const bell = {
    BoardRead.board,
    BoardRead.stations,
    BoardRead.heldBriefs,
    BoardRead.proposals,
    BoardRead.greenInk,
  };
  final (live, extra) = switch (page) {
    OoPage.overview || OoPage.memory || OoPage.pages => (
      const {BoardRead.board, BoardRead.stations},
      const {BoardRead.missions},
    ),
    OoPage.missions => (
      const {BoardRead.board, BoardRead.missions},
      const <BoardRead>{},
    ),
    OoPage.operations ||
    OoPage.sessions => (const {BoardRead.board}, const <BoardRead>{}),
    OoPage.stations => (const {BoardRead.stations}, const {BoardRead.missions}),
    _ => (const <BoardRead>{}, const <BoardRead>{}),
  };
  return (live: live, background: {...bell, ...extra}.difference(live));
}

/// When each repeated read is next due, given the page shown and whether the tab is.
class BoardCadence {
  BoardCadence({required this.every, required this.settings});

  /// The base interval, `[refresh] sessions`.
  final Duration every;
  final RefreshSettings settings;

  final _readAt = <BoardRead, DateTime>{};

  /// The page shown.
  OoPage page = OoPage.overview;

  /// Whether the tab or window can be seen.
  bool visible = true;

  /// Records that [reads] were asked for at [now].
  void read(Iterable<BoardRead> reads, DateTime now) {
    for (final read in reads) {
      _readAt[read] = now;
    }
  }

  /// Forgets every read, as after a sign-out.
  void forget() => _readAt.clear();

  /// What this page wants that has not been read within [every]: asked for at once when the
  /// page or the tab comes into view.
  Set<BoardRead> catchUp(DateTime now) {
    final wanted = readsOf(page);
    return {
      for (final read in {...wanted.live, ...wanted.background})
        if (_older(read, every, now)) read,
    };
  }

  /// What is due at [now].
  Set<BoardRead> due(DateTime now) {
    final wanted = readsOf(page);
    final slow = settings.slow;
    final profile = settings.profile;
    if (!visible) {
      final hidden = settings.hidden;
      if (hidden == null) return const {};
      return {
        for (final read in {...wanted.live, ...wanted.background})
          if (_older(read, hidden, now)) read,
        if (profile != null &&
            _older(BoardRead.profile, _longer(hidden, profile), now))
          BoardRead.profile,
      };
    }
    return {
      ...wanted.live,
      for (final read in wanted.background)
        if (slow == null || _older(read, slow, now)) read,
      if (profile != null && _older(BoardRead.profile, profile, now))
        BoardRead.profile,
    };
  }

  // A read counts as due a tenth of a second early: timers do not fire exactly on their
  // interval, and a due read missed by a millisecond would wait a whole tick.
  bool _older(BoardRead read, Duration age, DateTime now) {
    final at = _readAt[read];
    return at == null ||
        now.difference(at) >= age - const Duration(milliseconds: 100);
  }

  static Duration _longer(Duration a, Duration b) => a > b ? a : b;
}
