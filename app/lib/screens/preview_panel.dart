import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';

import '../format.dart';
import '../i18n.dart';
import '../models.dart';
import '../state.dart';
import '../theme.dart';
import '../widgets.dart';

class PreviewPanel extends StatefulWidget {
  final FileItem item;
  final Results results;
  const PreviewPanel({super.key, required this.item, required this.results});
  @override
  State<PreviewPanel> createState() => _PreviewPanelState();
}

class _PreviewPanelState extends State<PreviewPanel> {
  int tab = 0;
  String? imagePath;
  String? text;
  List<int>? hex;
  int hexOff = 0;
  bool loading = true;
  Map<String, dynamic>? detail;
  bool opening = false;

  FileItem get it => widget.item;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    final e = widget.results.session.engine!;
    final app = AppScope.read(context);
    try {
      detail = await e.call('entry', {'entry': it.id}) as Map<String, dynamic>;
    } catch (_) {}
    if (it.previewAsImage && it.chance > 3) {
      imagePath = await app.thumbs?.get(it.id, 900);
    }
    if (imagePath == null && it.previewAsText) {
      final b = await _read(0, 64 * 1024);
      text = utf8.decode(b, allowMalformed: true);
    }
    if (imagePath == null && text == null) {
      tab = 1;
      await _hex(0);
    }
    if (mounted) setState(() => loading = false);
  }

  Future<List<int>> _read(int off, int len) async {
    final v = await widget.results.session.engine!.call('read', {'entry': it.id, 'off': off, 'len': len}) as Map<String, dynamic>;
    final s = v['hex'] as String;
    return [for (var i = 0; i + 1 < s.length; i += 2) int.parse(s.substring(i, i + 2), radix: 16)];
  }

  Future<void> _hex(int off) async {
    hex = await _read(off, 512);
    hexOff = off;
    if (mounted) setState(() {});
  }

  Future<void> _open() async {
    setState(() => opening = true);
    final messenger = ScaffoldMessenger.of(context);
    try {
      final v = await widget.results.session.engine!.call('extract', {'entry': it.id}) as Map<String, dynamic>;
      final p = v['path'] as String?;
      if (p == null) throw t('prev.too_big');
      await Process.start('xdg-open', [p], mode: ProcessStartMode.detached);
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('${t('prev.open_fail')}: $e')));
    }
    if (mounted) setState(() => opening = false);
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final hasView = imagePath != null || text != null;
    return Padding(
      padding: const EdgeInsets.fromLTRB(0, 10, 12, 12),
      child: Material(
        color: cs.surfaceContainerLow,
        borderRadius: BorderRadius.circular(24),
        clipBehavior: Clip.antiAlias,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 8, 6, 0),
              child: Row(
                children: [
                  Expanded(child: Text(it.name, style: tx.titleMedium, maxLines: 2, overflow: TextOverflow.ellipsis)),
                  IconButton(onPressed: () => widget.results.focus(null), icon: const Icon(Icons.close_rounded)),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
              child: SegmentedButton<int>(
                showSelectedIcon: false,
                style: const ButtonStyle(visualDensity: VisualDensity.compact),
                segments: [
                  ButtonSegment(value: 0, label: Text(t('prev.preview')), enabled: hasView),
                  ButtonSegment(value: 1, label: Text(t('prev.hex'))),
                  ButtonSegment(value: 2, label: Text(t('prev.details'))),
                ],
                selected: {tab},
                onSelectionChanged: (v) {
                  setState(() => tab = v.first);
                  if (v.first == 1 && hex == null) _hex(0);
                },
              ),
            ),
            Expanded(
              child: KeyedSubtree(key: ValueKey('$tab$loading'), child: _content()),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 4, 12, 12),
              child: Row(
                children: [
                  Expanded(
                    child: OutlinedButton.icon(
                      onPressed: opening || it.chance <= 3 ? null : _open,
                      icon: const Icon(Icons.open_in_new_rounded),
                      label: Text(opening ? t('prev.preparing') : t('prev.open')),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: FilledButton.tonalIcon(
                      onPressed: () => widget.results.toggle(it),
                      icon: Icon(widget.results.isSelected(it) ? Icons.remove_done_rounded : Icons.done_all_rounded),
                      label: Text(widget.results.isSelected(it) ? t('prev.unpick') : t('prev.pick')),
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

  Widget _content() {
    if (loading) return const Center(child: CircularProgressIndicator());
    return switch (tab) {
      0 => _preview(),
      1 => _hexView(),
      _ => _details(),
    };
  }

  Widget _preview() {
    final cs = Theme.of(context).colorScheme;
    if (imagePath != null) {
      return Padding(
        padding: const EdgeInsets.all(12),
        child: Center(
          child: ClipRRect(
            borderRadius: BorderRadius.circular(16),
            child: InteractiveViewer(child: Image.file(File(imagePath!), fit: BoxFit.contain)),
          ),
        ),
      );
    }
    if (text != null) {
      return Container(
        margin: const EdgeInsets.all(12),
        padding: const EdgeInsets.all(12),
        decoration: BoxDecoration(color: cs.surfaceContainerHigh, borderRadius: BorderRadius.circular(16)),
        child: SingleChildScrollView(
          child: SelectableText(text!, style: const TextStyle(fontFamily: 'monospace', fontSize: 12, height: 1.4)),
        ),
      );
    }
    return EmptyState(icon: Icons.visibility_off_rounded, title: t('prev.none'));
  }

  Widget _hexView() {
    final cs = Theme.of(context).colorScheme;
    final b = hex;
    if (b == null) return const Center(child: CircularProgressIndicator());
    if (b.isEmpty) return Center(child: Text(t('prev.nodata')));
    final lines = <String>[];
    for (var i = 0; i < b.length; i += 12) {
      final row = b.sublist(i, (i + 12).clamp(0, b.length));
      final h = row.map((x) => x.toRadixString(16).padLeft(2, '0')).join(' ').padRight(36);
      final a = row.map((x) => x >= 32 && x < 127 ? String.fromCharCode(x) : '·').join();
      lines.add('${(hexOff + i).toRadixString(16).padLeft(6, '0')}  $h $a');
    }
    final zero = b.every((x) => x == 0);
    return Column(
      children: [
        if (zero)
          Padding(
            padding: EdgeInsets.fromLTRB(12, 0, 12, 6),
            child: InfoBanner(icon: Icons.info_rounded, title: t('prev.zero'), body: t('prev.zero_body')),
          ),
        Expanded(
          child: Container(
            margin: const EdgeInsets.symmetric(horizontal: 12),
            padding: const EdgeInsets.all(10),
            decoration: BoxDecoration(color: cs.surfaceContainerHigh, borderRadius: BorderRadius.circular(16)),
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: SingleChildScrollView(
                child: SelectableText(lines.join('\n'), style: const TextStyle(fontFamily: 'monospace', fontSize: 11, height: 1.45)),
              ),
            ),
          ),
        ),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            IconButton(onPressed: hexOff == 0 ? null : () => _hex((hexOff - 512).clamp(0, it.size)), icon: const Icon(Icons.chevron_left_rounded)),
            Text('${fmtCount(hexOff)} / ${fmtCount(it.size)}', style: Theme.of(context).textTheme.labelSmall),
            IconButton(onPressed: hexOff + 512 >= it.size ? null : () => _hex(hexOff + 512), icon: const Icon(Icons.chevron_right_rounded)),
          ],
        ),
      ],
    );
  }

  Widget _details() {
    final tx = Theme.of(context).textTheme;
    final cs = Theme.of(context).colorScheme;
    final st = Theme.of(context).extension<StatusColors>()!;
    final d = detail;
    Widget row(String k, String v) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 5),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(width: 104, child: Text(k, style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant))),
              Expanded(child: SelectableText(v, style: tx.bodySmall)),
            ],
          ),
        );
    final info = ChanceInfo.of(it.chance);
    final (bg, _) = st.tier(info.tier);
    return ListView(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      children: [
        Row(
          children: [
            ChanceBadge(chance: it.chance, dense: false),
            const SizedBox(width: 10),
            Expanded(
              child: TweenAnimationBuilder<double>(
                tween: Tween(begin: 0, end: it.chance / 100),
                duration: Motion.long,
                curve: Motion.enter,
                builder: (_, v, _) => LinearProgressIndicator(value: v, minHeight: 8, color: bg, borderRadius: BorderRadius.circular(4)),
              ),
            ),
            const SizedBox(width: 8),
            Text('${it.chance}%', style: tx.labelMedium),
          ],
        ),
        if (it.note != null)
          Padding(
            padding: const EdgeInsets.only(top: 10),
            child: InfoBanner(icon: Icons.info_rounded, title: tn(it.note)),
          ),
        const SizedBox(height: 8),
        row(t('info.state'), it.state.label),
        row(t('info.size'), '${fmtBytes(it.size)} (${fmtCount(it.size)} B)'),
        row(t('info.path'), it.path.isEmpty ? '—' : pathLabel(it.path)),
        row(t('info.mtime'), fmtDate(it.mtime)),
        row(t('info.ctime'), fmtDate(it.ctime)),
        if (it.dtime > 0) row(t('info.dtime'), fmtDate(it.dtime)),
        row(t('info.method'), it.method),
        if (d?['pieces'] != null) row(t('info.pieces'), '${d!['pieces']}'),
        if (d?['first_off'] != null) row(t('info.offset'), '${d!['first_off']}'),
      ],
    );
  }
}
