import 'dart:convert';
import 'dart:io';

import 'package:flutter/scheduler.dart';

/// SALVAGE_PERF=file: records frame times stage by stage. Invisible to users,
/// for developer measurement only.
class Perf {
  static final enabled = Platform.environment['SALVAGE_PERF'] != null;
  static final _frames = <String, List<(double, double)>>{};
  static String _stage = 'boot';

  static void start() {
    if (!enabled) return;
    SchedulerBinding.instance.addTimingsCallback((List<FrameTiming> list) {
      final b = _frames.putIfAbsent(_stage, () => []);
      for (final f in list) {
        b.add((f.buildDuration.inMicroseconds / 1000, f.rasterDuration.inMicroseconds / 1000));
      }
    });
  }

  static void stage(String s) => _stage = s;

  static double _pct(List<double> v, double p) {
    if (v.isEmpty) return 0;
    final s = [...v]..sort();
    return s[((s.length - 1) * p).round()];
  }

  static Future<void> finish() async {
    final out = <String, Object?>{};
    _frames.forEach((k, v) {
      final b = v.map((e) => e.$1).toList();
      final r = v.map((e) => e.$2).toList();
      out[k] = {
        'frame': v.length,
        'build_p50': _pct(b, .5),
        'build_p99': _pct(b, .99),
        'raster_p50': _pct(r, .5),
        'raster_p99': _pct(r, .99),
        'raster_max': _pct(r, 1),
      };
    });
    await File(Platform.environment['SALVAGE_PERF']!).writeAsString(const JsonEncoder.withIndent(' ').convert(out));
    exit(0);
  }
}
