import 'package:flutter/material.dart';

import '../engine.dart';
import '../format.dart';
import '../i18n.dart';
import '../state.dart';
import '../theme.dart';
import '../widgets.dart';
import 'tools.dart';

const appVersion = '2.1.2';

class HistoryPage extends StatelessWidget {
  const HistoryPage({super.key});

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final st = Theme.of(context).extension<StatusColors>()!;
    final kc = Theme.of(context).extension<KindColors>()!;
    return ListenableBuilder(
      listenable: Listenable.merge([app.history, app.settings]),
      builder: (context, _) {
        final items = app.history.items;
        final files = items.fold<int>(0, (a, b) => a + b.entries);
        final bytes = items.fold<int>(0, (a, b) => a + b.bytes);
        final tiles = [
          StatTile(icon: Icons.history_rounded, label: t('stat.scans'), value: Text(fmtCount(items.length)), bg: cs.secondaryContainer, fg: cs.onSecondaryContainer),
          StatTile(icon: Icons.manage_search_rounded, label: t('stat.found'), value: Text(fmtCount(files)), bg: kc.of(0).$1, fg: kc.of(0).$2),
          StatTile(icon: Icons.donut_large_rounded, label: t('stat.found_size'), value: Text(fmtBytes(bytes)), bg: kc.of(1).$1, fg: kc.of(1).$2),
          StatTile(icon: Icons.restore_page_rounded, label: t('stat.recovered'), value: Text(fmtCount(app.settings.recoveredFiles)), bg: st.good, fg: st.onGood),
        ];
        return ListView(
          padding: const EdgeInsets.all(20),
          children: [
            Padding(padding: const EdgeInsets.fromLTRB(4, 4, 0, 14), child: Text(t('nav.history'), style: tx.headlineMedium)),
            LayoutBuilder(builder: (context, c) {
              final n = c.maxWidth > 760 ? 4 : 2;
              return Wrap(spacing: 10, runSpacing: 10, children: [for (final w in tiles) SizedBox(width: (c.maxWidth - 10 * (n - 1)) / n, child: w)]);
            }),
            const SizedBox(height: 18),
            if (items.isEmpty)
              SizedBox(height: 320, child: EmptyState(icon: Icons.history_rounded, title: t('hist.empty'), body: t('hist.empty_body')))
            else
              for (var i = 0; i < items.length; i++)
                Padding(
                  padding: const EdgeInsets.only(bottom: 10),
                  child: StaggerIn(
                    index: i,
                    child: Pressable(
                      color: cs.surfaceContainerLow,
                      borderRadius: BorderRadius.circular(22),
                      onTap: () => app.reopen(items[i]),
                      child: Padding(
                        padding: const EdgeInsets.all(14),
                        child: Row(
                          children: [
                            Container(
                              width: 48,
                              height: 48,
                              decoration: BoxDecoration(color: cs.primaryContainer, borderRadius: BorderRadius.circular(16)),
                              child: Icon(Icons.folder_special_rounded, color: cs.onPrimaryContainer),
                            ),
                            const SizedBox(width: 14),
                            Expanded(
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Text(items[i].title, style: tx.titleMedium, overflow: TextOverflow.ellipsis),
                                  Text('${fmtDate(items[i].created)} · ${items[i].source}', style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant), overflow: TextOverflow.ellipsis),
                                ],
                              ),
                            ),
                            const SizedBox(width: 12),
                            Column(
                              crossAxisAlignment: CrossAxisAlignment.end,
                              children: [
                                Text(t('hist.files', {'n': fmtCount(items[i].entries)}), style: tx.titleSmall),
                                Text(fmtBytes(items[i].bytes), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant)),
                              ],
                            ),
                            const SizedBox(width: 6),
                            IconButton(tooltip: t('common.delete'), onPressed: () => app.history.remove(items[i]), icon: const Icon(Icons.delete_rounded)),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
          ],
        );
      },
    );
  }
}

class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});
  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  bool? passwordless;

  @override
  void initState() {
    super.initState();
    Engine.passwordless().then((v) {
      if (mounted) setState(() => passwordless = v);
    });
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final s = app.settings;
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    const palette = [0xFF7C5CFF, 0xFF2F6FED, 0xFF0F9D8A, 0xFF2E9E5B, 0xFFD9A400, 0xFFE2702B, 0xFFD93A4A, 0xFFC2459C];
    return ListenableBuilder(
      listenable: s,
      builder: (context, _) {
        final left = [
          _Section(title: t('set.look'), children: [
            ListTile(
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.language_rounded),
              title: Text(t('set.lang')),
              trailing: DropdownButton<String>(
                value: s.language,
                underline: const SizedBox(),
                borderRadius: BorderRadius.circular(14),
                menuMaxHeight: 420,
                items: [
                  DropdownMenuItem(value: 'system', child: Text('${t('set.lang_system')} (${L.systemLang().name})')),
                  for (final l in languages) DropdownMenuItem(value: l.code, child: Text(l.name)),
                ],
                onChanged: (v) => s.setLanguage(v!),
              ),
            ),
            ListTile(
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.dark_mode_rounded),
              title: Text(t('set.theme')),
              trailing: SegmentedButton<ThemeMode>(
                showSelectedIcon: false,
                segments: [
                  ButtonSegment(value: ThemeMode.light, label: Text(t('set.light'))),
                  ButtonSegment(value: ThemeMode.dark, label: Text(t('set.dark'))),
                  ButtonSegment(value: ThemeMode.system, label: Text(t('set.system'))),
                ],
                selected: {s.themeMode},
                onSelectionChanged: (v) => s.update((x) => x.themeMode = v.first),
              ),
            ),
            if (s.hasWallpaperTheme)
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                value: s.wallpaperAccent,
                onChanged: (v) => s.update((x) => x.wallpaperAccent = v),
                title: Text(t('set.wall')),
                subtitle: Text(t('set.wall_sub')),
              ),
            if (!(s.wallpaperAccent && s.hasWallpaperTheme))
              Padding(
                padding: const EdgeInsets.only(top: 8, bottom: 4),
                child: Wrap(
                  spacing: 10,
                  runSpacing: 10,
                  children: [
                    for (final c in palette)
                      InkWell(
                        borderRadius: BorderRadius.circular(22),
                        onTap: () => s.update((x) => x.seed = Color(c)),
                        child: AnimatedContainer(
                          duration: Motion.medium,
                          curve: Motion.spring,
                          width: 44,
                          height: 44,
                          decoration: BoxDecoration(
                            color: Color(c),
                            borderRadius: BorderRadius.circular(s.seed.toARGB32() == c ? 14 : 22),
                            border: s.seed.toARGB32() == c ? Border.all(color: cs.onSurface, width: 2) : null,
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              value: s.reduceMotion,
              onChanged: (v) => s.update((x) => x.reduceMotion = v),
              title: Text(t('set.motion')),
              subtitle: Text(t('set.motion_sub')),
            ),
          ]),
          const SizedBox(height: 14),
          _Section(title: t('set.scan'), children: [
            SwitchListTile(contentPadding: EdgeInsets.zero, value: s.autoSave, onChanged: (v) => s.update((x) => x.autoSave = v), title: Text(t('set.save')), subtitle: Text(t('set.save_sub'))),
            SwitchListTile(contentPadding: EdgeInsets.zero, value: s.verifyHash, onChanged: (v) => s.update((x) => x.verifyHash = v), title: Text(t('rec.verify')), subtitle: Text(t('rec.verify_sub'))),
            SwitchListTile(contentPadding: EdgeInsets.zero, value: s.notify, onChanged: (v) => s.update((x) => x.notify = v), title: Text(t('set.notify')), subtitle: Text(t('set.notify_sub'))),
          ]),
        ];
        final right = [
          _Section(title: t('set.security'), children: [
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              value: passwordless ?? false,
              onChanged: passwordless == null
                  ? null
                  : (v) async {
                      final ok = await Engine.setPasswordless(v);
                      final now = await Engine.passwordless();
                      if (mounted && ok) setState(() => passwordless = now);
                    },
              title: Text(t('set.nopass')),
              subtitle: Text(t('set.nopass_sub')),
            ),
            _Fact(Icons.lock_rounded, t('fact.ro'), t('fact.ro_sub')),
            _Fact(Icons.admin_panel_settings_rounded, t('fact.root'), t('fact.root_sub')),
            _Fact(Icons.wifi_off_rounded, t('fact.net'), t('fact.net_sub')),
          ]),
          const SizedBox(height: 14),
          _Section(title: t('set.about'), children: [
            Text('Salvage $appVersion', style: tx.titleSmall),
            const SizedBox(height: 2),
            Text(t('about.body'), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant, height: 1.4)),
            const SizedBox(height: 8),
            OutlinedButton.icon(onPressed: () => checkUpdates(context, appVersion), icon: const Icon(Icons.system_update_alt_rounded), label: Text(t('update.check'))),
          ]),
        ];
        return ListView(
          padding: const EdgeInsets.all(20),
          children: [
            Padding(padding: const EdgeInsets.fromLTRB(4, 4, 0, 14), child: Text(t('nav.settings'), style: tx.headlineMedium)),
            LayoutBuilder(
              builder: (context, c) => c.maxWidth >= 900
                  ? Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Expanded(child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: left)),
                        const SizedBox(width: 14),
                        Expanded(child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: right)),
                      ],
                    )
                  : Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [...left, const SizedBox(height: 14), ...right]),
            ),
          ],
        );
      },
    );
  }
}

class _Section extends StatelessWidget {
  final String title;
  final List<Widget> children;
  const _Section({required this.title, required this.children});

  @override
  Widget build(BuildContext context) {
    final tx = Theme.of(context).textTheme;
    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 16, 20, 14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(title, style: tx.titleMedium?.copyWith(color: Theme.of(context).colorScheme.primary)),
            const SizedBox(height: 6),
            ...children,
          ],
        ),
      ),
    );
  }
}

class _Fact extends StatelessWidget {
  final IconData icon;
  final String title, body;
  const _Fact(this.icon, this.title, this.body);

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 22, color: cs.onSurfaceVariant),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(title, style: tx.bodyLarge),
                Text(body, style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant)),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
