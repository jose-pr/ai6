import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import '../settings/oo_settings.g.dart';

/// One picture of an [OoArt], at a real size in pixels.
@immutable
class OoArtFile {
  const OoArtFile(this.asset, this.width, this.height);

  final String asset;
  final int width;
  final int height;
}

/// The most bytes a hero file of this [width] may take: the web downloads the one a screen
/// needs, and a native install carries them all.
int ooArtBudget(int width) => width <= 1536
    ? 130 * 1024
    : width <= 2560
    ? 350 * 1024
    : 700 * 1024;

/// The most bytes one wallpaper drawing may take.
const ooWallpaperBudget = 48 * 1024;

/// `ai6.cfg`'s `[oo.art]` settings.
@immutable
class OoArtLimits {
  const OoArtLimits({this.maxWidth = defaultMaxWidth});

  static const defaultMaxWidth = OoDefaults.artMaxWidth;

  /// `max_width`: the widest file ever fetched, for a site on slow links.
  final int maxWidth;

  /// The limits in force, set once at start from `ai6.cfg`.
  static OoArtLimits current = const OoArtLimits();
}

/// A picture in the sizes it ships at, widest last. Device pixel ratio alone cannot choose, since
/// a 4K panel at 100% has a ratio of 1, so the box's physical size does.
@immutable
class OoArt {
  const OoArt(this.files);

  final List<OoArtFile> files;

  /// What each picture was last chosen as, so a window made smaller fetches nothing new.
  static final _chosen = <OoArt, OoArtFile>{};

  /// Forgets the choices, for a test that starts a new session.
  @visibleForTesting
  static void forget() => _chosen.clear();

  /// The file for a box of [box] physical pixels: the smallest that covers it, by width or by
  /// height at the file's aspect (a phone's box is decided by height), else the largest within
  /// [maxWidth]. Once a larger file was chosen it stays for this session.
  OoArtFile pick(Size box, {int? maxWidth}) {
    final cap = maxWidth ?? OoArtLimits.current.maxWidth;
    final allowed = files.where((file) => file.width <= cap).toList();
    final choices = allowed.isEmpty ? [files.first] : allowed;
    final covers = choices.where(
      (file) =>
          file.width >= box.width &&
          box.height * file.width / file.height <= file.width,
    );
    final file = covers.isEmpty ? choices.last : covers.first;
    final earlier = _chosen[this];
    if (earlier != null &&
        earlier.width >= file.width &&
        choices.contains(earlier)) {
      return earlier;
    }
    _chosen[this] = file;
    return file;
  }

  /// The width to decode [file] at for a box of [box] physical pixels drawn with
  /// [BoxFit.cover]: enough to cover it, never more than the file holds.
  static int decodeWidth(OoArtFile file, Size box) {
    final needed = math.max(box.width, box.height * file.width / file.height);
    return math.max(1, math.min(file.width, needed.ceil()));
  }
}
