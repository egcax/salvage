import 'dart:io';

import 'package:flutter/material.dart';

import '../i18n.dart';
import '../models.dart';
import '../state.dart';
import '../theme.dart';
import '../widgets.dart';

void showScanSetup(BuildContext context, {required String source, required String title, DeviceNode? node, DeviceNode? disk, Set<FileKind> preset = const {}}) {
  showGeneralDialog(
    context: context,
    barrierDismissible: true,
    barrierLabel: t('common.close'),
    barrierColor: Colors.black38,
    transitionDuration: Motion.medium,
    pageBuilder: (_, _, _) => Align(
      alignment: AlignmentDirectional.centerEnd,
      child: _SetupSheet(source: source, title: title, node: node, disk: disk, preset: preset),
    ),
    transitionBuilder: (context, a, _, child) {
      final c = CurvedAnimation(parent: a, curve: Motion.enter, reverseCurve: Motion.exit);
      final dir = Directionality.of(context) == TextDirection.rtl ? -1.0 : 1.0;
      return SlideTransition(position: Tween(begin: Offset(0.6 * dir, 0), end: Offset.zero).animate(c), child: child);
    },
  );
}

class _Mode {
  final String key;
  final IconData icon;
  const _Mode(this.key, this.icon);
}

const _modes = [_Mode('quick', Icons.bolt_rounded), _Mode('deep', Icons.manage_search_rounded), _Mode('full', Icons.troubleshoot_rounded)];

class _Preset {
  final String key;
  final IconData icon;
  final Set<FileKind> kinds;
  const _Preset(this.key, this.icon, this.kinds);
}

const _presets = [
  _Preset('all', Icons.all_inclusive_rounded, {}),
  _Preset('photos', Icons.photo_library_rounded, {FileKind.image, FileKind.video}),
  _Preset('docs', Icons.article_rounded, {FileKind.document, FileKind.archive}),
  _Preset('media', Icons.library_music_rounded, {FileKind.audio, FileKind.video}),
];

class _SetupSheet extends StatefulWidget {
  final String source, title;
  final DeviceNode? node, disk;
  final Set<FileKind> preset;
  const _SetupSheet({required this.source, required this.title, this.node, this.disk, this.preset = const {}});
  @override
  State<_SetupSheet> createState() => _SetupSheetState();
}

class _SetupSheetState extends State<_SetupSheet> {
  String mode = 'deep';
  late Set<FileKind> kinds = {...widget.preset};
  bool existing = false;
  bool findLost = true;
  bool background = false;
  bool repair = true;
  double minKb = 0;
  double maxMb = 0;
  int step = 512;
  bool unmounting = false;
  late bool holdTrim = _discard;
  late bool rescue = _discard;

  bool get _discard => widget.node?.anyDiscard ?? false;

  List<DeviceNode> _mounted(DeviceNode? n) => n == null ? [] : [if (n.mounted) n, ...n.flat.where((c) => c.mounted)];

  bool _isSystem(List<DeviceNode> m) => m.any((x) => x.mountpoints.any((p) => p == '/' || p == '/boot' || p == '/home'));

  Future<void> _unmount(List<DeviceNode> list) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    setState(() => unmounting = true);
    for (final m in list) {
      final err = await app.devices.unmount(m.path);
      if (err != null) messenger.showSnackBar(SnackBar(content: Text('${t('setup.unmount_fail')}: $err')));
    }
    if (mounted) setState(() => unmounting = false);
  }

  void _start() {
    final app = AppScope.read(context);
    Navigator.of(context).pop();
    final uid = Platform.environment['XDG_RUNTIME_DIR'] ?? '/tmp';
    app.start(source: widget.source, title: widget.title, holdTrim: holdTrim && _discard, opts: {
      'mode': mode,
      'kinds': kinds.map((e) => e.key).toList(),
      'free_only': mode != 'full',
      'existing': existing,
      'find_lost': findLost,
      'step': step,
      'min_size': (minKb * 1024).toInt(),
      'max_rate': (maxMb * (1 << 20)).toInt(),
      'background': background,
      'repair': repair,
      if (rescue) 'rescue_dest': '$uid/salvage-rescue',
      if (rescue) 'rescue_limit': 2 << 30,
    });
  }

  Widget _switch(String title, String sub, bool v, ValueChanged<bool>? f) =>
      SwitchListTile(contentPadding: EdgeInsets.zero, dense: true, value: v, onChanged: f, title: Text(title), subtitle: Text(sub));

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final st = Theme.of(context).extension<StatusColors>()!;
    final live = app.devices.devices.expand((d) => [d, ...d.flat]).where((d) => d.path == widget.source).firstOrNull ?? widget.node;
    final mountedList = _mounted(live);
    final system = _isSystem(mountedList);
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Material(
        color: cs.surfaceContainerLow,
        borderRadius: BorderRadius.circular(28),
        clipBehavior: Clip.antiAlias,
        child: SizedBox(
          width: 500,
          child: Column(
            children: [
              Padding(
                padding: const EdgeInsetsDirectional.fromSTEB(24, 20, 12, 8),
                child: Row(
                  children: [
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(t('setup.title'), style: tx.headlineSmall),
                          Text(widget.title, style: tx.bodyMedium?.copyWith(color: cs.onSurfaceVariant), overflow: TextOverflow.ellipsis),
                        ],
                      ),
                    ),
                    IconButton(onPressed: () => Navigator.of(context).pop(), icon: const Icon(Icons.close_rounded)),
                  ],
                ),
              ),
              Expanded(
                child: ListView(
                  padding: const EdgeInsets.fromLTRB(20, 8, 20, 12),
                  children: [
                    if (mountedList.isNotEmpty && !system)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 12),
                        child: InfoBanner(
                          icon: Icons.warning_rounded,
                          color: st.fair,
                          onColor: st.onFair,
                          title: t('setup.mounted'),
                          body: t('setup.mounted_body'),
                          action: FilledButton.tonal(
                            onPressed: unmounting ? null : () => _unmount(mountedList),
                            child: Text(unmounting ? t('setup.unmounting') : t('setup.unmount')),
                          ),
                        ),
                      ),
                    if (system)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 12),
                        child: InfoBanner(
                          icon: Icons.emergency_rounded,
                          color: st.poor,
                          onColor: st.onPoor,
                          title: t('setup.system'),
                          body: _discard ? t('setup.system_trim') : t('setup.system_body'),
                        ),
                      ),
                    if (_discard) ...[
                      _switch(t('setup.hold_trim'), t('setup.hold_trim_sub'), holdTrim, (v) => setState(() => holdTrim = v)),
                      _switch(t('setup.rescue'), t('setup.rescue_sub'), rescue, (v) => setState(() => rescue = v)),
                      const SizedBox(height: 8),
                    ],
                    for (final m in _modes)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 8),
                        child: _ModeCard(mode: m, selected: mode == m.key, onTap: () => setState(() => mode = m.key)),
                      ),
                    const SizedBox(height: 10),
                    Text(t('setup.presets'), style: tx.titleSmall),
                    const SizedBox(height: 8),
                    Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      children: [
                        for (final p in _presets)
                          ChoiceChip(
                            avatar: Icon(p.icon, size: 18),
                            label: Text(t('preset.${p.key}')),
                            selected: kinds.length == p.kinds.length && kinds.containsAll(p.kinds),
                            onSelected: (_) => setState(() => kinds = {...p.kinds}),
                          ),
                      ],
                    ),
                    const SizedBox(height: 8),
                    Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      children: [
                        for (final k in FileKind.values)
                          FilterChip(
                            avatar: Icon(k.icon, size: 18),
                            label: Text(k.label),
                            selected: kinds.contains(k),
                            onSelected: (v) => setState(() => v ? kinds.add(k) : kinds.remove(k)),
                          ),
                      ],
                    ),
                    const SizedBox(height: 8),
                    Theme(
                      data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
                      child: ExpansionTile(
                        tilePadding: const EdgeInsets.symmetric(horizontal: 4),
                        childrenPadding: const EdgeInsets.symmetric(horizontal: 4),
                        title: Text(t('setup.advanced'), style: tx.titleSmall),
                        children: [
                          _switch(t('setup.existing'), t('setup.existing_sub'), existing, (v) => setState(() => existing = v)),
                          _switch(t('setup.lost'), t('setup.lost_sub'), findLost, mode == 'quick' ? null : (v) => setState(() => findLost = v)),
                          _switch(t('setup.repair'), t('setup.repair_sub'), repair, (v) => setState(() => repair = v)),
                          _switch(t('setup.background'), t('setup.background_sub'), background, (v) => setState(() => background = v)),
                          Row(
                            children: [
                              Expanded(child: Text(t('setup.precision'))),
                              DropdownButton<int>(
                                value: step,
                                underline: const SizedBox(),
                                borderRadius: BorderRadius.circular(14),
                                items: [
                                  DropdownMenuItem(value: 4096, child: Text(t('setup.step_4k'))),
                                  DropdownMenuItem(value: 512, child: Text(t('setup.step_512'))),
                                  DropdownMenuItem(value: 1, child: Text(t('setup.step_1'))),
                                ],
                                onChanged: (v) => setState(() => step = v!),
                              ),
                            ],
                          ),
                          _slider(t('setup.min_file'), minKb, 1024, 32, minKb == 0 ? t('common.unlimited') : '${minKb.toInt()} KB', (v) => setState(() => minKb = v)),
                          _slider(t('setup.speed'), maxMb, 500, 50, maxMb == 0 ? t('common.unlimited') : '${maxMb.toInt()} MB/s', (v) => setState(() => maxMb = v)),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
              Container(
                padding: const EdgeInsets.fromLTRB(20, 12, 20, 18),
                child: SizedBox(
                  width: double.infinity,
                  height: 52,
                  child: FilledButton.icon(
                    onPressed: _start,
                    icon: const Icon(Icons.play_arrow_rounded),
                    label: Text(t('setup.start'), style: tx.titleMedium?.copyWith(color: cs.onPrimary)),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _slider(String label, double v, double max, int div, String value, ValueChanged<double> f) => Row(
        children: [
          Flexible(child: Text(label)),
          Expanded(child: Slider(value: v, max: max, divisions: div, onChanged: f)),
          SizedBox(width: 76, child: Text(value, textAlign: TextAlign.end)),
        ],
      );
}

class _ModeCard extends StatelessWidget {
  final _Mode mode;
  final bool selected;
  final VoidCallback onTap;
  const _ModeCard({required this.mode, required this.selected, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final fg = selected ? cs.onPrimaryContainer : cs.onSurface;
    final r = BorderRadius.circular(selected ? 28 : 18);
    return AnimatedContainer(
      duration: Motion.medium,
      curve: Motion.enter,
      decoration: BoxDecoration(color: selected ? cs.primaryContainer : cs.surfaceContainerHigh, borderRadius: r),
      child: Pressable(
        onTap: onTap,
        borderRadius: r,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
          child: Row(
            children: [
              Icon(mode.icon, color: fg, size: 26),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Expanded(child: Text(t('mode.${mode.key}'), style: tx.titleMedium?.copyWith(color: fg))),
                        Text(t('mode.${mode.key}_time'), style: tx.labelMedium?.copyWith(color: fg.withValues(alpha: 0.75))),
                      ],
                    ),
                    const SizedBox(height: 2),
                    Text(t('mode.${mode.key}_body'), style: tx.bodySmall?.copyWith(color: fg.withValues(alpha: 0.82), height: 1.3)),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
