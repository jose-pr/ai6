import 'dart:io';
import 'dart:math' as math;

import 'package:flutter_svg/flutter_svg.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:outer_office/main.dart';
import 'package:outer_office/shell/sign_in_page.dart';
import 'package:outer_office/theme/oo_art.dart';
import 'package:outer_office/theme/oo_backdrop.dart';
import 'package:outer_office/theme/oo_theme.dart';

import 'contrast.dart';

void main() {
  test('text on the panel over the wallpaper reads in every theme', () {
    final failures = <String>[];
    for (final theme in OoThemeId.values) {
      final t = theme.tokens;
      final wallpaper = t.backdrop;
      if (wallpaper == null) continue;
      final over = [
        for (final color in wallpaper.colours)
          luminance(
            Color.alphaBlend(
              t.surface.withValues(alpha: ooArtPanelOpacity),
              color,
            ),
          ),
      ];
      final low = over.reduce(math.min);
      final high = over.reduce(math.max);
      for (final (name, color) in [
        ('text', t.text),
        ('muted', t.muted),
        ('accent', t.accent),
      ]) {
        final l = luminance(color);
        final worst = low <= l && l <= high
            ? 1.0
            : math.min(ratio(l, low), ratio(l, high));
        if (worst < 4.5) {
          failures.add(
            '${theme.label}: $name is ${worst.toStringAsFixed(2)}:1',
          );
        }
      }
    }
    expect(failures, isEmpty);
  });

  testWidgets('wallpaper off and high contrast draw no wallpaper', (
    tester,
  ) async {
    for (final tokens in [
      OoThemeId.nightOps.tokens.copyWith(clearBackdrop: true),
      OoThemeId.nightOps.tokens.highContrast(),
    ]) {
      await tester.pumpWidget(
        MaterialApp(
          theme: ooTheme(OoThemeId.nightOps).copyWith(extensions: [tokens]),
          home: const Scaffold(body: OoBackdrop(child: Text('b'))),
        ),
      );
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 50)),
      );
      await tester.pump();
      expect(find.byType(SvgPicture), findsNothing);
    }
  });

  group('the wallpaper drawing', () {
    test(
      'each drawing is within its budget, and the rasters are gone',
      () async {
        for (final asset in [ooWallpaperAsset, ooWallpaperLabelAsset]) {
          expect(
            (await File(asset).readAsBytes()).length,
            lessThanOrEqualTo(ooWallpaperBudget),
            reason: asset,
          );
        }
        expect(
          Directory('assets/backdrops').listSync().map((file) => file.path),
          everyElement(endsWith('.svg')),
        );
      },
    );

    test('it uses no font and every colour slot it asks for', () async {
      final drawing = await File(ooWallpaperAsset).readAsString();
      final label = await File(ooWallpaperLabelAsset).readAsString();
      expect(label, isNot(contains('<text')));
      expect(drawing, isNot(contains('<text')));
      expect(drawing, isNot(contains('<filter')));
      for (final slot in List.generate(8, (i) => '#a0000${i + 1}')) {
        expect(drawing, contains(slot));
      }
      expect(label, contains('#a00009'));
    });

    for (final theme in OoThemeId.values) {
      testWidgets('${theme.label} paints it, or nothing without a wallpaper', (
        tester,
      ) async {
        await tester.pumpWidget(
          MaterialApp(
            theme: ooTheme(theme),
            home: const Scaffold(body: OoBackdrop(child: Text('panel'))),
          ),
        );
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 50)),
        );
        await tester.pump();
        expect(
          find.byType(SvgPicture),
          theme.tokens.backdrop == null ? findsNothing : findsOneWidget,
        );
        expect(find.text('panel'), findsOneWidget);
      });
    }

    testWidgets('it paints on its own layer', (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          theme: ooTheme(OoThemeId.nightOps),
          home: const Scaffold(body: OoBackdrop(child: Text('panel'))),
        ),
      );
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 50)),
      );
      await tester.pump();
      expect(
        find.ancestor(
          of: find.byType(SvgPicture),
          matching: find.byType(RepaintBoundary),
        ),
        findsWidgets,
      );
    });
  });

  group('the sign-in label', () {
    Future<void> signInAt(WidgetTester tester, Size size) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final board = MissionBoard(
        apiOrigin: Uri.parse('https://oo.example'),
        pollInterval: null,
      );
      addTearDown(board.dispose);
      await tester.pumpWidget(
        OuterOfficeApp(board: board, desktopWindows: false),
      );
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 50)),
      );
      await tester.pump();
    }

    for (final width in const [
      320.0,
      390.0,
      600.0,
      768.0,
      900.0,
      1024.0,
      1100.0,
      1249.0,
      1438.0,
      1920.0,
      2560.0,
      3840.0,
    ]) {
      testWidgets('at $width it is whole, or not there', (tester) async {
        final height = width < 600 ? 844.0 : 840.0;
        await signInAt(tester, Size(width, height));
        final label = find.byKey(const ValueKey('wallpaper-label'));
        expect(tester.takeException(), isNull);
        if (label.evaluate().isEmpty) {
          expect(
            width,
            lessThan(1000),
            reason: 'it fits in any desktop window',
          );
          return;
        }
        final rect = tester.getRect(label);
        expect(rect.left, greaterThanOrEqualTo(0));
        expect(rect.bottom, lessThanOrEqualTo(height));
        expect(rect.top, greaterThanOrEqualTo(0));
        final panel = (width - SignInPage.panelWidth) / 2;
        expect(
          rect.right,
          lessThanOrEqualTo(panel),
          reason: 'clear of the panel',
        );
      });
    }
  });
}
