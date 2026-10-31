import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

import '../format.dart';
import '../i18n.dart';
import '../models.dart';
import '../state.dart';
import '../theme.dart';
import '../widgets.dart';
import 'scan_setup.dart';
import 'tools.dart';

Future<void> openImageFile(BuildContext context) async {
  final f = await openFile(acceptedTypeGroups: [
    XTypeGroup(label: t('home.image_files'), extensions: const ['img', 'dd', 'iso', 'bin', 'raw']),
    XTypeGroup(label: t('home.all_files')),
  ]);
  if (f == null || !context.mounted) return;
  showScanSetup(context, source: f.path, title: f.name, node: null);
}

/// Source selection: goes straight to the device if there is a single removable one.
void chooseSource(BuildContext context, {Set<FileKind> preset = const {}}) {
  final app = AppScope.read(context);
  final list = app.devices.devices;
  final usb = list.where((d) => d.removable).toList();
  if (usb.length == 1) {
    showScanSetup(context, source: usb.first.path, title: usb.first.title, node: usb.first, preset: preset);
    return;
  }
  showDialog<void>(
    context: context,
    builder: (c) {
      final cs = Theme.of(c).colorScheme;
      final tx = Theme.of(c).textTheme;
      final rows = [for (final d in list) ...[d, ...d.flat.where((p) => !p.locked)]];
      return AlertDialog(
        title: Text(t('pick.title')),
        contentPadding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
        content: SizedBox(
          width: 480,
          child: ListView(
            shrinkWrap: true,
            children: [
              for (final d in rows)
                Padding(
                  padding: const EdgeInsets.only(bottom: 4),
                  child: Pressable(
                    borderRadius: BorderRadius.circular(16),
                    onTap: () {
                      Navigator.pop(c);
                      showScanSetup(context, source: d.path, title: d.label ?? d.title, node: d, preset: preset);
                    },
                    child: Padding(
                      padding: EdgeInsetsDirectional.fromSTEB(d.children.isEmpty && d.type != 'disk' ? 28 : 8, 10, 8, 10),
                      child: Row(children: [
                        Icon(d.type == 'disk' ? d.icon : d.partIcon, color: cs.primary),
                        const SizedBox(width: 12),
                        Expanded(child: Text(d.label ?? d.title, style: tx.bodyLarge, overflow: TextOverflow.ellipsis)),
                        Text(fmtBytes(d.size), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant)),
                      ]),
                    ),
                  ),
                ),
            ],
          ),
        ),
        actions: [
          TextButton.icon(
            onPressed: () {
              Navigator.pop(c);
              openImageFile(context);
            },
            icon: const Icon(Icons.album_rounded),
            label: Text(t('home.open_image')),
          ),
          TextButton(onPressed: () => Navigator.pop(c), child: Text(t('common.cancel'))),
        ],
      );
    },
  );
}

/// Actions that need a device: applied directly if there is a single disk.
void pickDrive(BuildContext context, void Function(DeviceNode) action) {
  final list = AppScope.read(context).devices.devices;
  if (list.isEmpty) return;
  if (list.length == 1) return action(list.first);
  showDialog<void>(
    context: context,
    builder: (c) => SimpleDialog(
      title: Text(t('pick.title')),
      children: [
        for (final d in list)
          SimpleDialogOption(
            onPressed: () {
              Navigator.pop(c);
              action(d);
            },
            child: Row(children: [
              Icon(d.icon),
              const SizedBox(width: 12),
              Expanded(child: Text(d.title, overflow: TextOverflow.ellipsis)),
              Text(fmtBytes(d.size)),
            ]),
          ),
      ],
    ),
  );
}

class HomePage extends StatelessWidget {
  const HomePage({super.key});

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    return ListenableBuilder(
      listenable: Listenable.merge([app.devices, app.history, app.settings]),
      builder: (context, _) {
        return LayoutBuilder(builder: (context, c) {
          final wide = c.maxWidth >= 1040;
          final main = _Main(app: app);
          final side = _Side(app: app);
          return SingleChildScrollView(
            padding: const EdgeInsets.all(20),
            child: wide
                ? Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(child: main),
                      const SizedBox(width: 16),
                      SizedBox(width: 332, child: side),
                    ],
                  )
                : Column(children: [main, const SizedBox(height: 16), side]),
          );
        });
      },
    );
  }
}

class _Main extends StatelessWidget {
  final AppModel app;
  const _Main({required this.app});

  @override
  Widget build(BuildContext context) {
    final store = app.devices;
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final kinds = Theme.of(context).extension<KindColors>()!;
    final st = Theme.of(context).extension<StatusColors>()!;
    final cap = store.devices.fold<int>(0, (a, d) => a + d.size);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const _Hero(),
        const SizedBox(height: 14),
        LayoutBuilder(builder: (context, c) {
          final tiles = [
            StatTile(icon: Icons.storage_rounded, label: t('stat.drives'), value: RollingCount(value: store.devices.length, style: tx.titleLarge), bg: cs.secondaryContainer, fg: cs.onSecondaryContainer),
            StatTile(icon: Icons.donut_large_rounded, label: t('stat.capacity'), value: Text(fmtBytes(cap)), bg: kinds.of(0).$1, fg: kinds.of(0).$2),
            StatTile(icon: Icons.restore_page_rounded, label: t('stat.recovered'), value: RollingCount(value: app.settings.recoveredFiles, style: tx.titleLarge), bg: st.good, fg: st.onGood),
            StatTile(icon: Icons.history_rounded, label: t('stat.scans'), value: RollingCount(value: app.history.items.length, style: tx.titleLarge), bg: kinds.of(3).$1, fg: kinds.of(3).$2),
          ];
          final n = c.maxWidth > 760 ? 4 : 2;
          return Wrap(
            spacing: 10,
            runSpacing: 10,
            children: [for (final w in tiles) SizedBox(width: (c.maxWidth - 10 * (n - 1)) / n, child: w)],
          );
        }),
        const SizedBox(height: 20),
        SectionTitle(t('drives.title')),
        if (store.loading)
          const Padding(padding: EdgeInsets.all(40), child: Center(child: CircularProgressIndicator()))
        else if (store.error != null)
          EmptyState(icon: Icons.error_rounded, title: t('home.error'), body: store.error)
        else ...[
          for (var i = 0; i < store.devices.length; i++)
            Padding(
              padding: const EdgeInsets.only(bottom: 12),
              child: StaggerIn(key: ValueKey(store.devices[i].path), index: i, child: _DriveCard(device: store.devices[i], health: store.health[store.devices[i].path])),
            ),
          if (!store.devices.any((d) => d.removable)) const _PlugHint(),
          const SizedBox(height: 20),
          SectionTitle(t('home.quick')),
          _QuickActions(app: app),
        ],
      ],
    );
  }
}

class _Hero extends StatelessWidget {
  const _Hero();

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final fg = cs.onPrimaryContainer;
    return Container(
      height: 214,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(32),
        gradient: LinearGradient(colors: [cs.primaryContainer, cs.tertiaryContainer], begin: Alignment.topLeft, end: Alignment.bottomRight),
      ),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(32),
        child: Stack(
          fit: StackFit.expand,
          children: [
            RepaintBoundary(child: RadarArt(color: fg)),
            Padding(
              padding: const EdgeInsets.all(26),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 440),
                    child: Text(t('hero.title'), style: tx.headlineMedium?.copyWith(color: fg, fontWeight: FontWeight.w700), maxLines: 2),
                  ),
                  const SizedBox(height: 6),
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 440),
                    child: Text(t('hero.sub'), style: tx.bodyMedium?.copyWith(color: fg.withValues(alpha: 0.85)), maxLines: 2, overflow: TextOverflow.ellipsis),
                  ),
                  const SizedBox(height: 14),
                  Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      FilledButton.icon(onPressed: () => chooseSource(context), icon: const Icon(Icons.radar_rounded), label: Text(t('hero.start'))),
                      _QuickChip(icon: Icons.photo_library_rounded, label: t('preset.photos'), kinds: const {FileKind.image, FileKind.video}, fg: fg),
                      _QuickChip(icon: Icons.article_rounded, label: t('preset.docs'), kinds: const {FileKind.document, FileKind.archive}, fg: fg),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _QuickChip extends StatelessWidget {
  final IconData icon;
  final String label;
  final Set<FileKind> kinds;
  final Color fg;
  const _QuickChip({required this.icon, required this.label, required this.kinds, required this.fg});

  @override
  Widget build(BuildContext context) {
    return OutlinedButton.icon(
      style: OutlinedButton.styleFrom(foregroundColor: fg, side: BorderSide(color: fg.withValues(alpha: 0.35))),
      onPressed: () => chooseSource(context, preset: kinds),
      icon: Icon(icon, size: 18),
      label: Text(label),
    );
  }
}

class _QuickActions extends StatelessWidget {
  final AppModel app;
  const _QuickActions({required this.app});

  @override
  Widget build(BuildContext context) {
    final tx = Theme.of(context).textTheme;
    final cs = Theme.of(context).colorScheme;
    final kc = Theme.of(context).extension<KindColors>()!;
    final items = [
      (Icons.album_rounded, t('home.open_image'), kc.of(6), () => openImageFile(context)),
      (Icons.save_rounded, t('image.title'), kc.of(5), () => pickDrive(context, (d) => showImaging(context, d))),
      (Icons.monitor_heart_rounded, t('health.title'), kc.of(2), () => pickDrive(context, (d) => showHealth(context, d))),
      (Icons.history_rounded, t('nav.history'), kc.of(3), () => app.go(2)),
    ];
    return LayoutBuilder(builder: (context, c) {
      final n = c.maxWidth > 680 ? 4 : 2;
      return Wrap(
        spacing: 10,
        runSpacing: 10,
        children: [
          for (final (icon, label, tone, onTap) in items)
            SizedBox(
              width: (c.maxWidth - 10 * (n - 1)) / n,
              child: Pressable(
                color: cs.surfaceContainerLow,
                borderRadius: BorderRadius.circular(20),
                onTap: onTap,
                child: Padding(
                  padding: const EdgeInsets.all(14),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Container(
                        width: 38,
                        height: 38,
                        decoration: BoxDecoration(color: tone.$1, borderRadius: BorderRadius.circular(13)),
                        child: Icon(icon, size: 20, color: tone.$2),
                      ),
                      const SizedBox(height: 10),
                      Text(label, style: tx.labelLarge, maxLines: 2, overflow: TextOverflow.ellipsis),
                    ],
                  ),
                ),
              ),
            ),
        ],
      );
    });
  }
}

class _PlugHint extends StatelessWidget {
  const _PlugHint();

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
      decoration: BoxDecoration(borderRadius: BorderRadius.circular(22), border: Border.all(color: cs.outlineVariant)),
      child: Row(
        children: [
          Icon(Icons.usb_rounded, color: cs.primary, size: 24),
          const SizedBox(width: 12),
          Expanded(child: Text(t('home.plug'), style: tx.bodyMedium?.copyWith(color: cs.onSurfaceVariant))),
        ],
      ),
    );
  }
}

class _DriveCard extends StatelessWidget {
  final DeviceNode device;
  final bool? health;
  const _DriveCard({required this.device, this.health});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final d = device;
    final parts = d.flat;
    final palette = [cs.primary, cs.tertiary, cs.secondary, cs.error, cs.outline];
    final used = parts.fold<double>(0, (a, p) => a + ((p.fsTotal ?? 0) - (p.fsFree ?? 0)).toDouble().clamp(0, double.infinity));
    final known = parts.any((p) => p.fsTotal != null);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(18),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Donut(
              size: 96,
              thickness: 11,
              parts: known ? [(used, cs.primary), (d.size - used, Colors.transparent)] : [(1, cs.outlineVariant)],
              center: Icon(d.icon, size: 30, color: d.removable ? cs.primary : cs.onSurfaceVariant),
            ),
            const SizedBox(width: 18),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(d.title, style: tx.titleMedium, overflow: TextOverflow.ellipsis),
                            Text('${fmtBytes(d.size)} · ${d.transportLabel}', style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant), overflow: TextOverflow.ellipsis),
                          ],
                        ),
                      ),
                      _ChanceHint(device: d),
                      PopupMenuButton<String>(
                        tooltip: t('common.more'),
                        icon: const Icon(Icons.more_vert_rounded),
                        onSelected: (v) {
                          if (v == 'health') showHealth(context, d);
                          if (v == 'image') showImaging(context, d);
                        },
                        itemBuilder: (_) => [
                          PopupMenuItem(value: 'health', child: ListTile(leading: const Icon(Icons.monitor_heart_rounded), title: Text(t('health.title')), contentPadding: EdgeInsets.zero)),
                          PopupMenuItem(value: 'image', child: ListTile(leading: const Icon(Icons.save_rounded), title: Text(t('image.title')), contentPadding: EdgeInsets.zero)),
                        ],
                      ),
                    ],
                  ),
                  if (parts.isNotEmpty) ...[
                    const SizedBox(height: 8),
                    StackedBar(height: 10, parts: [for (var i = 0; i < parts.length; i++) (parts[i].size.toDouble(), palette[i % palette.length])]),
                    const SizedBox(height: 8),
                    for (var i = 0; i < parts.length; i++) _PartRow(disk: d, part: parts[i], color: palette[i % palette.length]),
                  ],
                  const SizedBox(height: 4),
                  Align(
                    alignment: AlignmentDirectional.centerEnd,
                    child: FilledButton.tonalIcon(
                      onPressed: () => showScanSetup(context, source: d.path, title: d.title, node: d),
                      icon: const Icon(Icons.radar_rounded, size: 18),
                      label: Text(t('home.scan_disk')),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ChanceHint extends StatelessWidget {
  final DeviceNode device;
  const _ChanceHint({required this.device});

  @override
  Widget build(BuildContext context) {
    final st = Theme.of(context).extension<StatusColors>()!;
    final d = device;
    final ssdInternal = !d.removable && !d.rotational && d.trim;
    final (Color bg, Color fg, String text, String tip) = d.anyDiscard
        ? (st.poor, st.onPoor, t('hint.trim'), t('hint.trim_tip'))
        : ssdInternal
            ? (st.fair, st.onFair, t('hint.ssd'), t('hint.ssd_tip'))
            : (st.good, st.onGood, t('hint.good'), t('hint.good_tip'));
    return Tooltip(
      message: tip,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
        decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(10)),
        child: Text(text, style: TextStyle(color: fg, fontSize: 12, fontWeight: FontWeight.w500)),
      ),
    );
  }
}

class _PartRow extends StatelessWidget {
  final DeviceNode disk, part;
  final Color color;
  const _PartRow({required this.disk, required this.part, required this.color});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final p = part;
    return InkWell(
      borderRadius: BorderRadius.circular(12),
      onTap: () => p.locked ? showUnlock(context, p) : showScanSetup(context, source: p.path, title: p.label ?? p.name, node: p, disk: disk),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 6),
        child: Row(
          children: [
            Container(width: 10, height: 10, decoration: BoxDecoration(color: color, borderRadius: BorderRadius.circular(4))),
            const SizedBox(width: 10),
            Expanded(
              child: Text.rich(
                TextSpan(children: [
                  TextSpan(text: p.label ?? p.name, style: tx.bodyMedium),
                  TextSpan(text: '   ${p.locked ? t('part.locked') : (p.mountedAt ?? t('part.unmounted'))}', style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant)),
                ]),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            const SizedBox(width: 8),
            Text((p.fstype ?? t('common.unknown')).toUpperCase(), style: tx.labelSmall?.copyWith(color: cs.onSurfaceVariant)),
            const SizedBox(width: 12),
            SizedBox(width: 62, child: Text(fmtBytes(p.size), textAlign: TextAlign.end, style: tx.bodySmall)),
            Icon(Icons.chevron_right_rounded, size: 18, color: cs.onSurfaceVariant),
          ],
        ),
      ),
    );
  }
}

class _Side extends StatelessWidget {
  final AppModel app;
  const _Side({required this.app});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final store = app.devices;
    final st = Theme.of(context).extension<StatusColors>()!;
    final bad = store.health.values.any((v) => !v);
    final known = store.health.isNotEmpty;
    final discard = store.devices.any((d) => d.anyDiscard);
    final snaps = store.snapshotConfigs.length;
    Widget row(IconData i, String title, String sub, (Color, Color) tone) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Row(
            children: [
              Container(
                width: 38,
                height: 38,
                decoration: BoxDecoration(color: tone.$1, borderRadius: BorderRadius.circular(13)),
                child: Icon(i, size: 20, color: tone.$2),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(title, style: tx.bodyMedium, maxLines: 1, overflow: TextOverflow.ellipsis),
                    Text(sub, style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant), maxLines: 2, overflow: TextOverflow.ellipsis),
                  ],
                ),
              ),
            ],
          ),
        );
    final recent = app.history.items.take(3).toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Card(
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(t('side.protect'), style: tx.titleMedium),
                const SizedBox(height: 6),
                row(Icons.favorite_rounded, t('prot.health'), !known ? t('prot.health_na') : (bad ? t('prot.health_bad') : t('prot.health_ok')), !known ? (cs.surfaceContainerHighest, cs.onSurfaceVariant) : (bad ? (st.lost, st.onLost) : (st.good, st.onGood))),
                row(Icons.bolt_rounded, 'TRIM', discard ? t('prot.trim_on') : t('prot.trim_off'), discard ? (st.poor, st.onPoor) : (st.good, st.onGood)),
                row(Icons.photo_camera_rounded, t('prot.snap'), snaps > 0 ? t('prot.snap_on', {'n': snaps}) : t('prot.snap_off'), snaps > 0 ? (st.good, st.onGood) : (st.fair, st.onFair)),
                row(Icons.lock_rounded, t('prot.ro'), t('prot.ro_on'), (st.good, st.onGood)),
              ],
            ),
          ),
        ),
        const SizedBox(height: 12),
        Card(
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(children: [
                  Expanded(child: Text(t('side.recent'), style: tx.titleMedium)),
                  if (app.history.items.isNotEmpty) TextButton(onPressed: () => app.go(2), child: Text(t('side.all'))),
                ]),
                const SizedBox(height: 4),
                if (recent.isEmpty)
                  Padding(padding: const EdgeInsets.symmetric(vertical: 8), child: Text(t('side.recent_none'), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant)))
                else
                  for (final r in recent)
                    InkWell(
                      borderRadius: BorderRadius.circular(12),
                      onTap: () => app.reopen(r),
                      child: Padding(
                        padding: const EdgeInsets.symmetric(vertical: 7, horizontal: 2),
                        child: Row(children: [
                          Icon(Icons.folder_special_rounded, size: 20, color: cs.primary),
                          const SizedBox(width: 10),
                          Expanded(
                            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                              Text(r.title, style: tx.bodyMedium, maxLines: 1, overflow: TextOverflow.ellipsis),
                              Text('${fmtDate(r.created, time: false)} · ${t('hist.files', {'n': fmtCount(r.entries)})}', style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant), maxLines: 1, overflow: TextOverflow.ellipsis),
                            ]),
                          ),
                        ]),
                      ),
                    ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 12),
        Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(color: cs.secondaryContainer, borderRadius: BorderRadius.circular(24)),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Icon(Icons.lightbulb_rounded, color: cs.onSecondaryContainer, size: 22),
              const SizedBox(width: 12),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(t('side.tip'), style: tx.titleSmall?.copyWith(color: cs.onSecondaryContainer)),
                  const SizedBox(height: 2),
                  Text(t('tip.body'), style: tx.bodySmall?.copyWith(color: cs.onSecondaryContainer, height: 1.35)),
                ]),
              ),
            ],
          ),
        ),
      ],
    );
  }
}
