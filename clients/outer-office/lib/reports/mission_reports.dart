import 'package:material_ui/material_ui.dart';

import '../held/held_briefs_page.dart' show revealInvisible;
import '../main.dart';
import '../routes/oo_route.dart';
import '../shell/oo_shell.dart';
import '../theme/oo_theme.dart';
import 'reports_model.dart';

/// A Mission's Reports: those it was begun for or resolves, and those its Agents filed while it
/// ran. Each opens on the Reports page.
class MissionReportsSection extends StatefulWidget {
  const MissionReportsSection({
    required this.board,
    required this.missionId,
    required this.pages,
    required this.onRoute,
    super.key,
  });

  final MissionBoard board;

  /// The Mission's UID, which a Report's link names.
  final String missionId;

  /// The pages this person may open; a Report opens only when Reports is one of them.
  final List<OoPage> pages;
  final ValueChanged<OoRoute> onRoute;

  @override
  State<MissionReportsSection> createState() => _MissionReportsSectionState();
}

class _MissionReportsSectionState extends State<MissionReportsSection> {
  late final Future<ReportsRead> _reports = widget.board.loadMissionReports(
    widget.missionId,
  );

  @override
  Widget build(BuildContext context) {
    final tokens = OoTokens.of(context);
    return FutureBuilder<ReportsRead>(
      future: _reports,
      builder: (context, snapshot) {
        if (snapshot.hasError) {
          // A person who may not read Reports has no use for this section.
          return snapshot.error is GatewayRefused &&
                  (snapshot.error as GatewayRefused).statusCode == 403
              ? const SizedBox.shrink()
              : Text(
                  'Reports could not be read.',
                  style: TextStyle(color: tokens.muted),
                );
        }
        final read = snapshot.data;
        if (read == null) return const LinearProgressIndicator();
        final during = [
          for (final report in read.reports)
            if (report.filedInMission == widget.missionId) report,
        ];
        final resolves = [
          for (final report in read.reports)
            if (report.filedInMission != widget.missionId) report,
        ];
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('REPORTS', style: ooSectionLabel(context)),
            const SizedBox(height: 6),
            if (read.reports.isEmpty)
              Text(
                'No Report is tied to this Mission.',
                style: TextStyle(color: tokens.muted),
              ),
            if (resolves.isNotEmpty)
              _Group(
                'It was begun for, or resolves',
                resolves,
                pages: widget.pages,
                onRoute: widget.onRoute,
              ),
            if (during.isNotEmpty)
              _Group(
                'Filed while it ran',
                during,
                pages: widget.pages,
                onRoute: widget.onRoute,
              ),
          ],
        );
      },
    );
  }
}

class _Group extends StatelessWidget {
  const _Group(
    this.title,
    this.reports, {
    required this.pages,
    required this.onRoute,
  });

  final String title;
  final List<ReportSummary> reports;
  final List<OoPage> pages;
  final ValueChanged<OoRoute> onRoute;

  @override
  Widget build(BuildContext context) {
    final text = Theme.of(context).textTheme;
    final tokens = OoTokens.of(context);
    final canOpen = pages.contains(OoPage.reports);
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title, style: text.titleSmall),
          for (final report in reports)
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton(
                onPressed: canOpen
                    ? () => onRoute(
                        const OoRoute(OoPage.reports)
                            .withField('report_id', report.id),
                      )
                    : null,
                child: Text.rich(
                  TextSpan(
                    children: [
                      TextSpan(
                        text: revealInvisible(
                          report.title ??
                              'A Report marked ${report.classification ?? 'above your clearance'}',
                        ),
                      ),
                      TextSpan(
                        text:
                            '  ${[?report.state, ?report.priority].join(' · ')}',
                        style: TextStyle(color: tokens.muted),
                      ),
                    ],
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}
