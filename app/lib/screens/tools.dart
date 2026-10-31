import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

import '../engine.dart';
import '../format.dart';
import '../i18n.dart';
import '../models.dart';
import '../state.dart';
import '../theme.dart';
import '../widgets.dart';

void showRecover(BuildContext context, ScanSession s, Results r) {
  showDialog<void>(context: context, barrierDismissible: false, builder: (_) => _RecoverDialog(session: s, results: r));
}

enum _Step { setup, running, done }

class _RecoverDialog extends StatefulWidget {
  final ScanSession session;
  final Results results;
  const _RecoverDialog({required this.session, required this.results});
  @override
  State<_RecoverDialog> createState() => _RecoverDialogState();
}

class _RecoverDialogState extends State<_RecoverDialog> {
  _Step step = _Step.setup;
  String dest = '';
  Map<String, dynamic>? check;
  bool riskAccepted = false;
  bool keepTree = true;
  bool skipLost = true;
  bool restoreTime = true;
  late bool verify;
  Map<String, dynamic>? progress;
  Map<String, dynamic>? finish;
  StreamSubscription<Map<String, dynamic>>? _sub;

  @override
  void initState() {
    super.initState();
    final st = AppScope.read(context).settings;
    verify = st.verifyHash;
    final home = Platform.environment['HOME'] ?? '/tmp';
    dest = st.defaultDest.isNotEmpty && Directory(st.defaultDest).existsSync() ? st.defaultDest : home;
    _check();
    _sub = widget.session.taskEvents.listen((m) {
      if (m['ev'] == 'recover') setState(() => progress = m);
      if (m['ev'] == 'recover_done') {
        setState(() {
          finish = m;
          step = _Step.done;
        });
      }
    });
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }

  Future<void> _check() async {
    try {
      final v = await widget.session.engine!.call('check_dest', {'path': dest}) as Map<String, dynamic>;
      if (mounted) setState(() => check = v);
    } catch (_) {}
  }

  Future<void> _pick() async {
    final p = await getDirectoryPath(initialDirectory: dest, confirmButtonText: t('rec.here'));
    if (p == null) return;
    setState(() {
      dest = p;
      check = null;
      riskAccepted = false;
    });
    _check();
  }

  bool get _samePart => check?['same_partition'] == true;
  bool get _sameDisk => check?['same_disk'] == true;
  int? get _free => (check?['free'] as num?)?.toInt();
  bool get _noRoom => _free != null && _free! < widget.results.selectionBytes;
  bool get _canStart => check != null && (!_samePart || riskAccepted) && !_noRoom;

  Future<void> _start() async {
    setState(() => step = _Step.running);
    AppScope.read(context).settings.update((s) => s.defaultDest = dest);
    try {
      await widget.session.engine!.call('recover', {
        'sel': widget.results.selection,
        'opts': {'dest': dest, 'keep_tree': keepTree, 'by_kind': !keepTree, 'verify': verify, 'skip_lost': skipLost, 'restore_time': restoreTime, 'report': true},
      });
    } catch (e) {
      setState(() {
        finish = {'error': e.toString()};
        step = _Step.done;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return Dialog(
      child: SizedBox(
        width: 540,
        child: switch (step) {
          _Step.setup => _setup(),
          _Step.running => _running(),
          _Step.done => _done(),
        },
      ),
    );
  }

  Widget _sw(String title, String? sub, bool v, ValueChanged<bool> f) =>
      SwitchListTile(contentPadding: EdgeInsets.zero, dense: true, value: v, onChanged: f, title: Text(title), subtitle: sub == null ? null : Text(sub));

  Widget _setup() {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final st = Theme.of(context).extension<StatusColors>()!;
    final r = widget.results;
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(t('rec.title'), style: tx.headlineSmall),
          const SizedBox(height: 2),
          Text(t('browse.selected', {'n': fmtCount(r.selectionCount), 'b': fmtBytes(r.selectionBytes)}), style: tx.bodyMedium?.copyWith(color: cs.onSurfaceVariant)),
          const SizedBox(height: 16),
          Pressable(
            color: cs.surfaceContainerHighest,
            onTap: _pick,
            child: Padding(
              padding: const EdgeInsets.all(14),
              child: Row(
                children: [
                  Icon(Icons.folder_open_rounded, color: cs.primary),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(t('rec.dest'), style: tx.labelMedium?.copyWith(color: cs.onSurfaceVariant)),
                        Text(dest, style: tx.bodyMedium, overflow: TextOverflow.ellipsis),
                        if (_free != null) Text(t('rec.free', {'b': fmtBytes(_free!)}), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant)),
                      ],
                    ),
                  ),
                  const Icon(Icons.edit_rounded),
                ],
              ),
            ),
          ),
          if (_samePart) ...[
            const SizedBox(height: 10),
            InfoBanner(
              icon: Icons.dangerous_rounded,
              color: st.lost,
              onColor: st.onLost,
              title: t('rec.same'),
              body: t('rec.same_body'),
              action: CheckboxListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                value: riskAccepted,
                onChanged: (v) => setState(() => riskAccepted = v ?? false),
                title: Text(t('rec.accept'), style: TextStyle(color: st.onLost)),
              ),
            ),
          ] else if (_sameDisk) ...[
            const SizedBox(height: 10),
            InfoBanner(icon: Icons.warning_rounded, color: st.fair, onColor: st.onFair, title: t('rec.same_disk'), body: t('rec.same_disk_body')),
          ],
          if (_noRoom) ...[
            const SizedBox(height: 10),
            InfoBanner(icon: Icons.storage_rounded, color: st.lost, onColor: st.onLost, title: t('rec.no_room')),
          ],
          const SizedBox(height: 8),
          _sw(t('rec.tree'), t('rec.tree_sub'), keepTree, (v) => setState(() => keepTree = v)),
          _sw(t('rec.verify'), t('rec.verify_sub'), verify, (v) => setState(() => verify = v)),
          _sw(t('rec.skip'), t('rec.skip_sub'), skipLost, (v) => setState(() => skipLost = v)),
          _sw(t('rec.dates'), null, restoreTime, (v) => setState(() => restoreTime = v)),
          const SizedBox(height: 12),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              TextButton(onPressed: () => Navigator.pop(context), child: Text(t('common.cancel'))),
              const SizedBox(width: 8),
              FilledButton.icon(onPressed: _canStart ? _start : null, icon: const Icon(Icons.restore_page_rounded), label: Text(t('rec.start'))),
            ],
          ),
        ],
      ),
    );
  }

  Widget _running() {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final p = progress;
    final total = (p?['total_bytes'] as num?)?.toDouble() ?? 0;
    final done = (p?['done_bytes'] as num?)?.toDouble() ?? 0;
    final v = total > 0 ? (done / total).clamp(0.0, 1.0) : null;
    return Padding(
      padding: const EdgeInsets.all(28),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          WavyRing(value: v, size: 148, child: v == null ? Icon(Icons.restore_page_rounded, size: 40, color: cs.primary) : Text('${(v * 100).floor()}%', style: tx.headlineMedium)),
          const SizedBox(height: 16),
          Text(t('rec.running'), style: tx.titleLarge),
          const SizedBox(height: 2),
          Text(p?['current'] as String? ?? t('rec.preparing'), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant), maxLines: 1, overflow: TextOverflow.ellipsis),
          const SizedBox(height: 14),
          if (p != null)
            Row(
              mainAxisAlignment: MainAxisAlignment.spaceEvenly,
              children: [
                Stat(t('rec.files'), Text('${fmtCount(p['done_files'] as num)} / ${fmtCount(p['total_files'] as num)}')),
                Stat(t('session.speed'), Text(fmtRate((p['rate'] as num).toDouble()))),
                Stat(t('session.eta'), Text(fmtDuration((p['eta'] as num).toDouble()))),
              ],
            ),
          const SizedBox(height: 18),
          OutlinedButton(onPressed: widget.session.cancelTask, child: Text(t('session.stop'))),
        ],
      ),
    );
  }

  Widget _done() {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final f = finish ?? {};
    final err = f['error'] as String?;
    final path = f['dest'] as String?;
    final ok = (f['ok'] as num?)?.toInt() ?? 0;
    final failed = (f['failed'] as num?)?.toInt() ?? 0;
    final skipped = (f['skipped'] as num?)?.toInt() ?? 0;
    final partial = (f['partial'] as num?)?.toInt() ?? 0;
    return Padding(
      padding: const EdgeInsets.all(28),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          SuccessMark(ok: err == null),
          const SizedBox(height: 14),
          Text(err ?? (f['cancelled'] == true ? t('rec.cancelled') : t('rec.done', {'n': fmtCount(ok)})), style: tx.headlineSmall, textAlign: TextAlign.center),
          if (err == null) ...[
            const SizedBox(height: 4),
            Text(
              [
                fmtBytes((f['bytes'] as num?) ?? 0),
                if (failed > 0) t('rec.failed', {'n': failed}),
                if (partial > 0) t('rec.partial', {'n': partial}),
                if (skipped > 0) t('rec.skipped', {'n': skipped}),
              ].join(' · '),
              style: tx.bodyMedium?.copyWith(color: cs.onSurfaceVariant),
            ),
            if (path != null) ...[
              const SizedBox(height: 6),
              Text(path, style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant), textAlign: TextAlign.center),
            ],
          ],
          const SizedBox(height: 18),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              TextButton(
                onPressed: () {
                  widget.results.clearSelection();
                  Navigator.pop(context);
                },
                child: Text(t('common.close')),
              ),
              if (path != null) ...[
                const SizedBox(width: 8),
                FilledButton.icon(
                  onPressed: () => Process.start('xdg-open', [path], mode: ProcessStartMode.detached),
                  icon: const Icon(Icons.folder_open_rounded),
                  label: Text(t('common.open_folder')),
                ),
              ],
            ],
          ),
        ],
      ),
    );
  }
}

/// SMART is read with a separate permission that needs no password.
Future<void> showHealth(BuildContext context, DeviceNode d) async {
  final messenger = ScaffoldMessenger.of(context);
  var smart = await Engine.smart(d.path);
  if (smart == null && Engine.installed) {
    // Old install or no permission: try through the engine.
    try {
      final e = await Engine.open(source: d.path, elevate: d.path.startsWith('/dev/'));
      smart = (await e.call('info') as Map<String, dynamic>)['smart'] as Map<String, dynamic>?;
      e.close();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(e.toString())));
      return;
    }
  }
  if (!context.mounted) return;
  if (smart == null) {
    messenger.showSnackBar(SnackBar(content: Text(t('health.none'))));
    return;
  }
  showSmart(context, smart);
}

void showSmart(BuildContext context, Map<String, dynamic> s) {
  final passed = (s['smart_status'] as Map?)?['passed'] as bool?;
  final temp = (s['temperature'] as Map?)?['current'];
  final hours = (s['power_on_time'] as Map?)?['hours'];
  final cycles = s['power_cycle_count'];
  final nvme = s['nvme_smart_health_information_log'] as Map?;
  final attrs = ((s['ata_smart_attributes'] as Map?)?['table'] as List?) ?? const [];
  num? raw(int id) {
    for (final a in attrs) {
      if ((a as Map)['id'] == id) return (a['raw'] as Map)['value'] as num?;
    }
    return null;
  }

  final rows = <(String, String)>[
    (t('health.model'), '${s['model_name'] ?? t('common.unknown')}'),
    if (s['serial_number'] != null) (t('health.serial'), '${s['serial_number']}'),
    if (temp != null) (t('health.temp'), '$temp °C'),
    if (hours != null) (t('health.hours'), '${fmtCount(hours as num)} h'),
    if (cycles != null) (t('health.cycles'), fmtCount(cycles as num)),
    if (nvme?['percentage_used'] != null) (t('health.wear'), '${nvme!['percentage_used']}%'),
    if (nvme?['available_spare'] != null) (t('health.spare'), '${nvme!['available_spare']}%'),
    if (nvme?['data_units_written'] != null) (t('health.written'), fmtBytes((nvme!['data_units_written'] as num) * 512000)),
    if (nvme?['unsafe_shutdowns'] != null) (t('health.unsafe'), fmtCount(nvme!['unsafe_shutdowns'] as num)),
    if (nvme?['media_errors'] != null) (t('health.media'), fmtCount(nvme!['media_errors'] as num)),
    if (raw(5) != null) (t('health.realloc'), fmtCount(raw(5)!)),
    if (raw(197) != null) (t('health.pending'), fmtCount(raw(197)!)),
    if (raw(198) != null) (t('health.uncorrect'), fmtCount(raw(198)!)),
  ];
  showDialog<void>(
    context: context,
    builder: (c) {
      final cs = Theme.of(c).colorScheme;
      final tx = Theme.of(c).textTheme;
      final st = Theme.of(c).extension<StatusColors>()!;
      final (bg, fg) = passed == false ? (st.lost, st.onLost) : (st.good, st.onGood);
      return AlertDialog(
        icon: Icon(Icons.monitor_heart_rounded, color: cs.primary),
        title: Text(t('health.title')),
        content: SizedBox(
          width: 440,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (passed != null)
                InfoBanner(
                  icon: passed ? Icons.verified_rounded : Icons.dangerous_rounded,
                  color: bg,
                  onColor: fg,
                  title: passed ? t('health.ok') : t('health.fail'),
                  body: passed ? null : t('health.fail_body'),
                ),
              const SizedBox(height: 8),
              for (final (k, v) in rows)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 3),
                  child: Row(children: [
                    Expanded(child: Text(k, style: tx.bodyMedium?.copyWith(color: cs.onSurfaceVariant))),
                    Flexible(child: Text(v, style: tx.bodyMedium, textAlign: TextAlign.end)),
                  ]),
                ),
            ],
          ),
        ),
        actions: [TextButton(onPressed: () => Navigator.pop(c), child: Text(t('common.close')))],
      );
    },
  );
}

void showImaging(BuildContext context, DeviceNode d) {
  showDialog<void>(context: context, barrierDismissible: false, builder: (_) => _ImagingDialog(device: d));
}

class _ImagingDialog extends StatefulWidget {
  final DeviceNode device;
  const _ImagingDialog({required this.device});
  @override
  State<_ImagingDialog> createState() => _ImagingDialogState();
}

class _ImagingDialogState extends State<_ImagingDialog> {
  String? dest;
  Engine? eng;
  Map<String, dynamic>? p;
  Map<String, dynamic>? finish;
  String? error;
  StreamSubscription<Map<String, dynamic>>? _sub;

  Future<void> _pick() async {
    final loc = await getSaveLocation(suggestedName: '${widget.device.name}.img', acceptedTypeGroups: [XTypeGroup(label: t('home.image_files'), extensions: const ['img'])]);
    if (loc != null) setState(() => dest = loc.path);
  }

  Future<void> _start() async {
    try {
      final e = await Engine.open(source: widget.device.path, elevate: true);
      eng = e;
      _sub = e.events.listen((m) {
        if (m['ev'] == 'image') setState(() => p = m);
        if (m['ev'] == 'image_done') setState(() => finish = m);
      });
      await e.call('image', {'dest': dest});
      setState(() => p = {'pass': 1, 'done': 0, 'total': widget.device.size, 'rate': 0, 'eta': -1, 'bad': 0});
    } catch (e) {
      setState(() => error = e.toString());
    }
  }

  @override
  void dispose() {
    _sub?.cancel();
    eng?.close();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tx = Theme.of(context).textTheme;
    final cs = Theme.of(context).colorScheme;
    final total = (p?['total'] as num?)?.toDouble() ?? 1;
    final done = (p?['done'] as num?)?.toDouble() ?? 0;
    final running = p != null && finish == null && error == null;
    final bad = (finish?['bad_bytes'] as num?) ?? 0;
    return AlertDialog(
      icon: Icon(Icons.save_rounded, color: cs.primary),
      title: Text(t('image.title')),
      content: SizedBox(
        width: 460,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(t('image.body', {'d': widget.device.title, 's': fmtBytes(widget.device.size)}), style: tx.bodyMedium?.copyWith(color: cs.onSurfaceVariant)),
            const SizedBox(height: 14),
            if (error != null)
              InfoBanner(icon: Icons.error_rounded, title: error!)
            else if (finish != null)
              InfoBanner(
                icon: Icons.task_alt_rounded,
                title: finish!['error'] as String? ?? (finish!['cancelled'] == true ? t('image.paused') : t('image.ready')),
                body: [if (bad > 0) t('image.bad', {'b': fmtBytes(bad)}), if (finish!['path'] != null) finish!['path'] as String].join('\n'),
              )
            else if (running) ...[
              if ((p!['pass'] as num) > 1) Text(t('image.retry'), style: tx.labelMedium),
              LinearProgressIndicator(value: (done / total).clamp(0, 1), minHeight: 8, borderRadius: BorderRadius.circular(4)),
              const SizedBox(height: 8),
              Text('${fmtBytes(done)} / ${fmtBytes(total)} · ${fmtRate((p!['rate'] as num).toDouble())} · ${fmtDuration((p!['eta'] as num).toDouble())}', style: tx.bodySmall),
              if ((p!['bad'] as num) > 0) Text(t('image.bad_live', {'n': p!['bad']}), style: TextStyle(color: cs.error)),
            ] else
              OutlinedButton.icon(onPressed: _pick, icon: const Icon(Icons.folder_open_rounded), label: Text(dest ?? t('image.pick'))),
          ],
        ),
      ),
      actions: [
        if (running)
          TextButton(onPressed: () => eng?.call('cancel_task'), child: Text(t('session.stop')))
        else
          TextButton(onPressed: () => Navigator.pop(context), child: Text(t('common.close'))),
        if (!running && finish == null && error == null) FilledButton(onPressed: dest == null ? null : _start, child: Text(t('common.start'))),
      ],
    );
  }
}

void showUnlock(BuildContext context, DeviceNode d) {
  final c = TextEditingController();
  final app = AppScope.read(context);
  showDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      icon: const Icon(Icons.lock_open_rounded),
      title: Text(t('unlock.title')),
      content: SizedBox(
        width: 380,
        child: TextField(controller: c, obscureText: true, autofocus: true, decoration: InputDecoration(labelText: t('unlock.pass'), border: const OutlineInputBorder())),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(ctx), child: Text(t('common.cancel'))),
        FilledButton(
          onPressed: () async {
            final m = ScaffoldMessenger.of(ctx);
            final err = await app.devices.unlock(d.path, c.text);
            c.clear();
            if (ctx.mounted) Navigator.pop(ctx);
            if (err != null) m.showSnackBar(SnackBar(content: Text(err)));
          },
          child: Text(t('unlock.go')),
        ),
      ],
    ),
  );
}

/// Shared progress dialog for long bulk operations.
Future<Map<String, dynamic>?> _taskDialog(BuildContext context, ScanSession s, String title, Future<void> Function() start) {
  return showDialog<Map<String, dynamic>>(
    context: context,
    barrierDismissible: false,
    builder: (ctx) => _TaskDialog(session: s, title: title, start: start),
  );
}

class _TaskDialog extends StatefulWidget {
  final ScanSession session;
  final String title;
  final Future<void> Function() start;
  const _TaskDialog({required this.session, required this.title, required this.start});
  @override
  State<_TaskDialog> createState() => _TaskDialogState();
}

class _TaskDialogState extends State<_TaskDialog> {
  int done = 0, total = 0;
  StreamSubscription<Map<String, dynamic>>? _sub;

  @override
  void initState() {
    super.initState();
    _sub = widget.session.taskEvents.listen((m) {
      if (m['ev'] == 'task') {
        setState(() {
          done = (m['done'] as num).toInt();
          total = (m['total'] as num).toInt();
        });
      }
      if (m['ev'] == 'task_done' && mounted) Navigator.pop(context, m);
    });
    widget.start().catchError((Object e) {
      if (mounted) {
        Navigator.pop(context);
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('$e')));
      }
    });
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.title),
      content: SizedBox(
        width: 400,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            LinearProgressIndicator(value: total > 0 ? done / total : null, minHeight: 8, borderRadius: BorderRadius.circular(4)),
            const SizedBox(height: 8),
            Text(total > 0 ? '${fmtCount(done)} / ${fmtCount(total)}' : '…'),
          ],
        ),
      ),
      actions: [TextButton(onPressed: widget.session.cancelTask, child: Text(t('session.stop')))],
    );
  }
}

Future<void> showGrep(BuildContext context, ScanSession s, Results r) async {
  final c = TextEditingController();
  final text = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      icon: const Icon(Icons.find_in_page_rounded),
      title: Text(t('tools.grep')),
      content: SizedBox(
        width: 420,
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          Text(t('grep.body')),
          const SizedBox(height: 12),
          TextField(controller: c, autofocus: true, onSubmitted: (v) => Navigator.pop(ctx, v), decoration: InputDecoration(labelText: t('grep.label'), border: const OutlineInputBorder())),
        ]),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(ctx), child: Text(t('common.cancel'))),
        FilledButton(onPressed: () => Navigator.pop(ctx, c.text), child: Text(t('grep.go'))),
      ],
    ),
  );
  if (text == null || text.trim().isEmpty || !context.mounted) return;
  final res = await _taskDialog(context, s, t('tools.grep'), () => s.engine!.call('grep', {'text': text.trim()}));
  if (res == null) return;
  r.change((x) => x.grepOnly = true);
  if (context.mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(t('grep.hits', {'n': fmtCount(res['hits'] as num)}))));
}

Future<void> showValidate(BuildContext context, ScanSession s, Results r) async {
  final sel = r.selectionCount > 0 ? r.selection : {'filter': r.filter};
  final res = await _taskDialog(context, s, t('tools.validate'), () => s.engine!.call('validate', {'sel': sel}));
  if (res == null) return;
  await r.refresh();
  if (context.mounted) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(t('validate.done', {'g': fmtCount(res['good'] as num), 'b': fmtCount(res['bad'] as num), 'u': fmtCount(res['unknown'] as num)}))));
  }
}

Future<void> exportList(BuildContext context, ScanSession s, Results r, String fmt) async {
  final messenger = ScaffoldMessenger.of(context);
  final loc = await getSaveLocation(suggestedName: 'salvage.$fmt');
  if (loc == null) return;
  try {
    final v = await s.engine!.call('export', {'filter': r.filter, 'path': loc.path, 'format': fmt}) as Map<String, dynamic>;
    messenger.showSnackBar(SnackBar(content: Text(t('export.done', {'n': fmtCount(v['rows'] as num)}))));
  } catch (e) {
    messenger.showSnackBar(SnackBar(content: Text('$e')));
  }
}

void showShortcuts(BuildContext context) {
  final items = [
    ('Ctrl+F', t('keys.search')),
    ('Ctrl+A', t('keys.all')),
    ('Ctrl+R', t('keys.recover')),
    ('Ctrl+G', t('keys.grid')),
    ('Esc', t('keys.esc')),
    ('F1', t('keys.help')),
  ];
  showDialog<void>(
    context: context,
    builder: (c) => AlertDialog(
      icon: const Icon(Icons.keyboard_rounded),
      title: Text(t('tools.keys')),
      content: SizedBox(
        width: 380,
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          for (final (k, v) in items)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 4),
              child: Row(children: [
                Container(
                  width: 78,
                  padding: const EdgeInsets.symmetric(vertical: 3),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(color: Theme.of(c).colorScheme.surfaceContainerHighest, borderRadius: BorderRadius.circular(8)),
                  child: Text(k, style: const TextStyle(fontFamily: 'monospace', fontSize: 12.5)),
                ),
                const SizedBox(width: 14),
                Expanded(child: Text(v)),
              ]),
            ),
        ]),
      ),
      actions: [TextButton(onPressed: () => Navigator.pop(c), child: Text(t('common.close')))],
    ),
  );
}

/// Runs only when the user presses it; nothing goes online in the background.
Future<void> checkUpdates(BuildContext context, String current) async {
  final messenger = ScaffoldMessenger.of(context);
  try {
    final c = HttpClient()..connectionTimeout = const Duration(seconds: 8);
    final q = await c.getUrl(Uri.parse('https://api.github.com/repos/egcax/salvage/releases/latest'));
    q.headers.set('User-Agent', 'Salvage');
    final r = await q.close();
    final body = await r.transform(utf8.decoder).join();
    c.close();
    if (r.statusCode != 200) throw t('update.none_yet');
    final tag = (jsonDecode(body) as Map<String, dynamic>)['tag_name'] as String? ?? '';
    final v = tag.replaceFirst('v', '');
    messenger.showSnackBar(SnackBar(
      content: Text(v == current ? t('update.latest') : t('update.new', {'v': v})),
      action: v == current
          ? null
          : SnackBarAction(label: t('update.open'), onPressed: () => Process.start('xdg-open', ['https://github.com/egcax/salvage/releases/latest'], mode: ProcessStartMode.detached)),
    ));
  } catch (e) {
    messenger.showSnackBar(SnackBar(content: Text('$e')));
  }
}
