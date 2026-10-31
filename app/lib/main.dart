import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import 'i18n.dart';
import 'perf.dart';
import 'state.dart';
import 'theme.dart';
import 'screens/home.dart';
import 'screens/other.dart';
import 'screens/scan_setup.dart';
import 'screens/session.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  Perf.start();
  final model = AppModel();
  await model.settings.load();
  Motion.reduced = model.settings.reduceMotion;
  runApp(AppScope(model: model, child: const SalvageApp()));
  await model.initRest();
  _handleArgs(model, args);
  if (Perf.enabled) unawaited(_perfScript(model));
}

/// salvage --open FILE [--mode quick|deep|full]: scans an image directly.
void _handleArgs(AppModel m, List<String> args) {
  final i = args.indexOf('--open');
  if (i < 0 || i + 1 >= args.length) return;
  final j = args.indexOf('--mode');
  final mode = j >= 0 && j + 1 < args.length ? args[j + 1] : 'deep';
  final path = args[i + 1];
  m.start(source: path, title: path.split('/').last, opts: {'mode': mode, 'free_only': mode != 'full', 'step': 512, 'find_lost': true});
}

/// SALVAGE_PERF: walks through page transitions automatically and writes frame times.
Future<void> _perfScript(AppModel m) async {
  Future<void> step(String name, FutureOr<void> Function() f, [int ms = 700]) async {
    Perf.stage(name);
    await f();
    await Future<void>.delayed(Duration(milliseconds: ms));
  }

  await Future<void>.delayed(const Duration(seconds: 2));
  final pages = m.session != null ? [0, 1, 2, 3] : [0, 2, 3];
  for (var k = 0; k < 6; k++) {
    for (final p in pages) {
      await step('page_$p', () => m.go(p));
    }
  }
  for (var k = 0; k < 6; k++) {
    m.go(0);
    await Future<void>.delayed(const Duration(milliseconds: 400));
    await step('side_sheet', () => showScanSetup(navKey.currentContext!, source: '/tmp/x', title: 'x'), 700);
    await step('side_sheet_close', () => navKey.currentState!.pop(), 500);
  }
  if (m.results != null) {
    m.go(1);
    await Future<void>.delayed(const Duration(seconds: 1));
    final r = m.results!;
    for (var k = 0; k < 12; k++) {
      final it = r.itemAt(k);
      if (it != null) await step('preview', () => r.focus(it), 450);
      await step('preview_close', () => r.focus(null), 250);
    }
    for (var k = 0; k < 6; k++) {
      await step('grid', () => r.setGrid(k.isEven), 500);
    }
  }
  Perf.stage('son');
  await Perf.finish();
}

final navKey = GlobalKey<NavigatorState>();

class SalvageApp extends StatelessWidget {
  const SalvageApp({super.key});

  @override
  Widget build(BuildContext context) {
    // Only settings are listened to: a page change must not rebuild the whole app.
    final app = AppScope.read(context);
    return ListenableBuilder(
      listenable: app.settings,
      builder: (context, _) {
        final seed = app.settings.activeSeed;
        Motion.reduced = app.settings.reduceMotion;
        return MaterialApp(
          title: 'Salvage',
          debugShowCheckedModeBanner: false,
          themeMode: app.settings.themeMode,
          theme: themeFor(seed, Brightness.light),
          darkTheme: themeFor(seed, Brightness.dark),
          themeAnimationDuration: Motion.medium,
          themeAnimationCurve: Motion.standard,
          locale: L.current.locale,
          supportedLocales: [for (final l in languages) l.locale],
          localizationsDelegates: const [
            GlobalMaterialLocalizations.delegate,
            GlobalWidgetsLocalizations.delegate,
            GlobalCupertinoLocalizations.delegate,
          ],
          // When the language changes, subtrees with fixed text are rebuilt too.
          navigatorKey: navKey,
          home: KeyedSubtree(key: ValueKey(L.tag), child: const Shell()),
        );
      },
    );
  }
}

class Shell extends StatelessWidget {
  const Shell({super.key});

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final cs = Theme.of(context).colorScheme;
    final hasSession = app.session != null;
    final pages = <int>[0, if (hasSession) 1, 2, 3];
    final selected = pages.indexOf(app.page).clamp(0, pages.length - 1);
    final dark = Theme.of(context).brightness == Brightness.dark;
    return Scaffold(
      backgroundColor: cs.surfaceContainerLow,
      body: Row(
        children: [
          RepaintBoundary(
            child: NavigationRail(
              selectedIndex: selected,
              onDestinationSelected: (i) => app.go(pages[i]),
              minWidth: 88,
              leading: Padding(
                padding: const EdgeInsets.fromLTRB(0, 14, 0, 20),
                child: FloatingActionButton(
                  elevation: 0,
                  tooltip: t('rail.new'),
                  onPressed: () {
                    app.go(0);
                    chooseSource(navKey.currentContext!);
                  },
                  child: const Icon(Icons.add_rounded),
                ),
              ),
              groupAlignment: -1,
              trailing: Expanded(
                child: Align(
                  alignment: Alignment.bottomCenter,
                  child: Padding(
                    padding: const EdgeInsets.only(bottom: 14),
                    child: IconButton(
                      tooltip: t('rail.theme'),
                      onPressed: () => app.settings.update((s) => s.themeMode = dark ? ThemeMode.light : ThemeMode.dark),
                      icon: Icon(dark ? Icons.light_mode_rounded : Icons.dark_mode_rounded),
                    ),
                  ),
                ),
              ),
              destinations: [
                NavigationRailDestination(icon: const Icon(Icons.home_outlined), selectedIcon: const Icon(Icons.home_rounded), label: Text(t('nav.home'))),
                if (hasSession)
                  NavigationRailDestination(icon: const _SessionIcon(), selectedIcon: const _SessionIcon(), label: Text(t('nav.scan'))),
                NavigationRailDestination(icon: const Icon(Icons.history_rounded), selectedIcon: const Icon(Icons.history_rounded), label: Text(t('nav.history'))),
                NavigationRailDestination(icon: const Icon(Icons.settings_outlined), selectedIcon: const Icon(Icons.settings_rounded), label: Text(t('nav.settings'))),
              ],
            ),
          ),
          Expanded(
            child: Padding(
              padding: const EdgeInsetsDirectional.fromSTEB(0, 8, 8, 8),
              // Rounded corner decoration: clipping the whole page was expensive on every frame.
              child: Stack(
                fit: StackFit.expand,
                children: [
                  DecoratedBox(decoration: BoxDecoration(color: cs.surface, borderRadius: BorderRadius.circular(28))),
                  _PageHost(page: app.page, hasSession: hasSession),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Visited pages stay alive: they are only repainted on a transition, not rebuilt.
/// The motion is a slide alone; a full-screen opacity layer (saveLayer) took a
/// single frame to 200 ms on integrated GPUs at high resolution.
class _PageHost extends StatefulWidget {
  final int page;
  final bool hasSession;
  const _PageHost({required this.page, required this.hasSession});
  @override
  State<_PageHost> createState() => _PageHostState();
}

class _PageHostState extends State<_PageHost> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: Motion.medium, value: 1);
  late final CurvedAnimation _curve = CurvedAnimation(parent: _c, curve: Motion.enter);
  final _built = <int>{};
  int _dir = 1;

  @override
  void didUpdateWidget(_PageHost old) {
    super.didUpdateWidget(old);
    if (old.page != widget.page) {
      _dir = widget.page > old.page ? 1 : -1;
      _c.duration = Motion.medium;
      _c.forward(from: 0);
    }
  }

  @override
  void dispose() {
    _curve.dispose();
    _c.dispose();
    super.dispose();
  }

  Widget _pageOf(int p) => switch (p) {
        0 => const HomePage(),
        1 => const SessionPage(),
        2 => const HistoryPage(),
        _ => const SettingsPage(),
      };

  @override
  Widget build(BuildContext context) {
    if (!widget.hasSession) _built.remove(1);
    _built.add(widget.page);
    return Stack(
      fit: StackFit.expand,
      children: [
        for (final p in _built)
          Offstage(
            key: ValueKey(p),
            offstage: p != widget.page,
            child: TickerMode(
              enabled: p == widget.page,
              // The tree must stay the same in every state: a changed branch rebuilds the page.
              child: AnimatedBuilder(
                animation: _curve,
                builder: (_, child) => Transform.translate(offset: Offset(0, p == widget.page ? 28.0 * _dir * (1 - _curve.value) : 0), child: child),
                child: RepaintBoundary(child: _pageOf(p)),
              ),
            ),
          ),
      ],
    );
  }
}

class _SessionIcon extends StatelessWidget {
  const _SessionIcon();

  @override
  Widget build(BuildContext context) {
    final s = AppScope.of(context).session;
    if (s == null) return const Icon(Icons.manage_search_rounded);
    return ListenableBuilder(
      listenable: s,
      builder: (_, _) => Badge(isLabelVisible: s.running, smallSize: 8, child: const Icon(Icons.manage_search_rounded)),
    );
  }
}
