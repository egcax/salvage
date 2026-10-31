import 'dart:io';

import 'package:flutter/material.dart';

import '../format.dart';
import '../i18n.dart';
import '../state.dart';
import '../theme.dart';
import '../widgets.dart';
import 'browser.dart';
import 'tools.dart';

/// The top bar listens to the session; the browser listens to its own data. This way
/// the progress events arriving 5 times a second do not rebuild the file list.
class SessionPage extends StatelessWidget {
  const SessionPage({super.key});

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final s = app.session;
    if (s == null) {
      return EmptyState(icon: Icons.manage_search_rounded, title: t('session.none'), body: t('home.plug'));
    }
    final r = app.results;
    return Column(
      children: [
        RepaintBoundary(child: _Top(session: s, app: app)),
        if (r != null) Expanded(child: RepaintBoundary(child: Browser(key: ObjectKey(r), results: r, session: s))),
      ],
    );
  }
}

class _Top extends StatelessWidget {
  final ScanSession session;
  final AppModel app;
  const _Top({required this.session, required this.app});

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: session,
      builder: (context, _) {
        final s = session;
        if (s.phase == ScanPhase.connecting) {
          return SizedBox(
            height: 300,
            child: EmptyState(
              icon: Icons.key_rounded,
              title: t('session.auth'),
              body: s.elevated ? t('session.auth_body') : t('session.opening'),
              action: const SizedBox(width: 160, child: LinearProgressIndicator()),
            ),
          );
        }
        if (s.phase == ScanPhase.failed && app.results == null) {
          return SizedBox(
            height: 320,
            child: EmptyState(
              icon: Icons.error_rounded,
              title: t('session.fail'),
              body: s.error,
              action: FilledButton.tonal(onPressed: app.closeSession, child: Text(t('common.close'))),
            ),
          );
        }
        return AnimatedSwitcher(
          duration: Motion.medium,
          child: s.running ? _LiveHeader(key: const ValueKey('l'), session: s) : _DoneHeader(key: const ValueKey('d'), session: s),
        );
      },
    );
  }
}

class _LiveHeader extends StatelessWidget {
  final ScanSession session;
  const _LiveHeader({super.key, required this.session});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final s = session;
    final paused = s.phase == ScanPhase.paused;
    final pct = s.fraction;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 20, 20, 8),
      child: Card(
        color: cs.surfaceContainer,
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: SizedBox(
            height: 150,
            child: Row(
              children: [
                WavyRing(
                  value: pct,
                  size: 148,
                  active: !paused,
                  child: pct == null
                      ? Icon(Icons.radar_rounded, size: 40, color: cs.primary)
                      : Text('${(pct * 100).floor()}%', style: tx.headlineMedium?.copyWith(fontFeatures: const [FontFeature.tabularFigures()])),
                ),
                const SizedBox(width: 24),
                Expanded(
                  flex: 5,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      AnimatedSwitcher(
                        duration: Motion.short,
                        child: Text(paused ? t('session.paused') : t(s.phaseName), key: ValueKey(paused ? 'p' : s.phaseName), style: tx.titleLarge, maxLines: 1, overflow: TextOverflow.ellipsis),
                      ),
                      Text(s.title, style: tx.bodyMedium?.copyWith(color: cs.onSurfaceVariant), overflow: TextOverflow.ellipsis),
                      const SizedBox(height: 14),
                      Wrap(
                        spacing: 28,
                        runSpacing: 8,
                        children: [
                          Stat(t('session.found'), RollingCount(value: s.found, style: tx.titleLarge)),
                          Stat(t('session.size'), RollingCount(value: s.foundBytes, style: tx.titleLarge, format: fmtBytes)),
                          if (s.total > (1 << 24)) Stat(t('session.speed'), Text(fmtRate(s.rate))),
                          if (s.eta >= 0) Stat(t('session.eta'), Text(fmtDuration(s.eta))),
                          if (s.errors > 0) Stat(t('session.errors'), Text(fmtCount(s.errors), style: TextStyle(color: cs.error))),
                        ],
                      ),
                      if (s.trimHeld.isNotEmpty)
                        Padding(
                          padding: const EdgeInsets.only(top: 6),
                          child: Row(children: [
                            Icon(Icons.shield_rounded, size: 15, color: cs.primary),
                            const SizedBox(width: 4),
                            Flexible(child: Text(t('session.trim_held'), style: tx.labelSmall?.copyWith(color: cs.primary), overflow: TextOverflow.ellipsis)),
                          ]),
                        ),
                    ],
                  ),
                ),
                const SizedBox(width: 20),
                Expanded(
                  flex: 4,
                  child: Column(
                    children: [
                      Expanded(child: DiskMap(cells: s.map)),
                      const SizedBox(height: 8),
                      const _Legend(),
                    ],
                  ),
                ),
                const SizedBox(width: 16),
                Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    IconButton.filledTonal(
                      tooltip: paused ? t('session.resume') : t('session.pause'),
                      onPressed: () => s.pause(!paused),
                      icon: Icon(paused ? Icons.play_arrow_rounded : Icons.pause_rounded),
                    ),
                    const SizedBox(height: 8),
                    IconButton.filled(
                      style: IconButton.styleFrom(backgroundColor: cs.errorContainer, foregroundColor: cs.onErrorContainer),
                      tooltip: t('session.stop'),
                      onPressed: s.cancelScan,
                      icon: const Icon(Icons.stop_rounded),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Legend extends StatelessWidget {
  const _Legend();

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme.labelSmall?.copyWith(color: cs.onSurfaceVariant);
    Widget dot(Color c, String s) => Row(mainAxisSize: MainAxisSize.min, children: [
          Container(width: 9, height: 9, decoration: BoxDecoration(color: c, borderRadius: BorderRadius.circular(3))),
          const SizedBox(width: 4),
          Text(s, style: tx),
        ]);
    return Wrap(spacing: 10, children: [
      dot(cs.primary.withValues(alpha: 0.55), t('map.data')),
      dot(cs.outlineVariant, t('map.empty')),
      dot(cs.tertiary, t('map.found')),
      dot(cs.secondary, t('map.meta')),
    ]);
  }
}

class _DoneHeader extends StatelessWidget {
  final ScanSession session;
  const _DoneHeader({super.key, required this.session});

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final st = Theme.of(context).extension<StatusColors>()!;
    final s = session;
    final z = s.volumes.map((v) => v.zeroRatio).whereType<double>().fold<double?>(null, (a, b) => a == null || b > a ? b : a);
    final fs = s.volumes.map((v) => v.label.isEmpty ? v.fstype : '${v.label} (${v.fstype})').join(', ');
    final mode = s.scanOpts?['mode'] ?? 'deep';
    final canDeep = s.phase != ScanPhase.loaded && mode != 'full';
    final lostVols = s.volumes.where((v) => v.lost).length;
    final note = s.volumes.map((v) => v.note).whereType<String>().where((n) => n.startsWith('note.')).firstOrNull;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 48,
                height: 48,
                decoration: BoxDecoration(color: cs.primaryContainer, borderRadius: BorderRadius.circular(16)),
                child: Icon(s.phase == ScanPhase.cancelled ? Icons.stop_circle_rounded : Icons.task_alt_rounded, color: cs.onPrimaryContainer),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                        s.phase == ScanPhase.loaded
                            ? t('session.saved_title', {'t': s.title})
                            : t('session.found_title', {'n': fmtCount(s.found), 'b': fmtBytes(s.foundBytes)}),
                        style: tx.titleLarge,
                        overflow: TextOverflow.ellipsis),
                    Text(
                      [
                        s.title,
                        if (fs.isNotEmpty) fs,
                        if (s.elapsed > 0) fmtDuration(s.elapsed),
                        if (lostVols > 0) t('session.lost_vols', {'n': lostVols}),
                        if (s.errors > 0) t('session.read_errors', {'n': fmtCount(s.errors)}),
                      ].join(' · '),
                      style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant),
                      overflow: TextOverflow.ellipsis,
                    ),
                  ],
                ),
              ),
              if (s.resumable)
                FilledButton.tonalIcon(onPressed: app.resume, icon: const Icon(Icons.play_arrow_rounded), label: Text(t('session.continue'))),
              if (canDeep && !s.resumable)
                TextButton.icon(
                  onPressed: () => s.scan({...?s.scanOpts, 'mode': mode == 'quick' ? 'deep' : 'full', 'free_only': mode == 'quick', 'rescue_dest': null}),
                  icon: const Icon(Icons.manage_search_rounded),
                  label: Text(mode == 'quick' ? t('session.go_deep') : t('session.go_full')),
                ),
              if (s.info?['smart'] != null)
                IconButton(tooltip: t('health.title'), onPressed: () => showSmart(context, s.info!['smart'] as Map<String, dynamic>), icon: const Icon(Icons.monitor_heart_rounded)),
              IconButton(tooltip: t('session.log'), onPressed: () => _logs(context, s), icon: const Icon(Icons.article_rounded)),
              IconButton(tooltip: t('session.close'), onPressed: app.closeSession, icon: const Icon(Icons.close_rounded)),
            ],
          ),
          if (s.rescuedDest != null)
            Padding(
              padding: const EdgeInsets.only(top: 10),
              child: InfoBanner(
                icon: Icons.health_and_safety_rounded,
                title: t('session.rescued', {'n': fmtCount(s.rescued)}),
                body: t('session.rescued_body'),
                action: FilledButton.tonal(
                  onPressed: () => Process.start('xdg-open', [s.rescuedDest!], mode: ProcessStartMode.detached),
                  child: Text(t('common.open_folder')),
                ),
              ),
            ),
          if (note != null) Padding(padding: const EdgeInsets.only(top: 10), child: InfoBanner(icon: Icons.info_rounded, title: tn(note))),
          if (z != null && z > 0.85 && s.source.startsWith('/dev/nvme'))
            Padding(
              padding: const EdgeInsets.only(top: 10),
              child: InfoBanner(
                icon: Icons.info_rounded,
                color: st.fair,
                onColor: st.onFair,
                title: t('session.zeroed', {'p': (z * 100).round()}),
                body: t('session.zeroed_body'),
              ),
            ),
        ],
      ),
    );
  }

  void _logs(BuildContext context, ScanSession s) {
    showDialog<void>(
      context: context,
      builder: (c) => AlertDialog(
        title: Text(t('session.log')),
        content: SizedBox(
          width: 620,
          height: 360,
          child: s.logs.isEmpty
              ? Center(child: Text(t('session.log_empty')))
              : SelectionArea(
                  child: ListView(children: [
                    for (final l in s.logs) Padding(padding: const EdgeInsets.symmetric(vertical: 3), child: Text(l, style: const TextStyle(fontFamily: 'monospace', fontSize: 12.5))),
                  ]),
                ),
        ),
        actions: [TextButton(onPressed: () => Navigator.pop(c), child: Text(t('common.close')))],
      ),
    );
  }
}
