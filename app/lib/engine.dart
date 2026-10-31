import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'i18n.dart';

class EngineError implements Exception {
  final String message;
  EngineError(this.message);
  @override
  String toString() => message;
}

/// Client that speaks JSON lines with the privileged engine.
class Engine {
  final Process _proc;
  final _events = StreamController<Map<String, dynamic>>.broadcast();
  final _pending = <int, Completer<dynamic>>{};
  int _next = 1;
  bool _closed = false;

  Engine._(this._proc);

  Stream<Map<String, dynamic>> get events => _events.stream;

  static String get binary {
    final env = Platform.environment['SALVAGE_ENGINE'];
    if (env != null && env.isNotEmpty) return env;
    final near = File('${File(Platform.resolvedExecutable).parent.path}/salvage-engine');
    if (near.existsSync()) return near.path;
    return '/opt/salvage/salvage-engine';
  }

  static Future<Map<String, dynamic>> devices() async {
    final r = await Process.run(binary, ['devices'], stdoutEncoding: utf8);
    if (r.exitCode != 0) throw EngineError('${r.stderr}');
    return jsonDecode(r.stdout as String) as Map<String, dynamic>;
  }

  static Future<Map<String, dynamic>?> sessionInfo(String path) async {
    final r = await Process.run(binary, ['session-info', '--session', path], stdoutEncoding: utf8);
    if (r.exitCode != 0) return null;
    return jsonDecode(r.stdout as String) as Map<String, dynamic>;
  }

  /// Block devices open through pkexec: the system shows the password window.
  /// The engine drops root right after opening the device.
  static Future<Engine> open({String? source, String? session, required bool elevate, bool holdTrim = false}) async {
    final args = [
      'serve',
      if (source != null) ...['--source', source],
      if (session != null) ...['--session', session],
      if (holdTrim) '--hold-trim',
    ];
    final Process p;
    try {
      p = elevate ? await Process.start('pkexec', [binary, ...args]) : await Process.start(binary, args);
    } on ProcessException catch (e) {
      throw EngineError(e.message);
    }
    final eng = Engine._(p);
    final ready = Completer<void>();
    eng._listen(ready);
    p.exitCode.then((code) {
      eng._closed = true;
      if (!ready.isCompleted) {
        final msg = switch (code) {
          126 => t('err.auth_cancel'),
          127 => t('err.auth_fail'),
          _ => t('err.engine', {'c': code}),
        };
        ready.completeError(EngineError(msg));
      }
      for (final c in eng._pending.values) {
        if (!c.isCompleted) c.completeError(EngineError(t('err.closed')));
      }
      eng._pending.clear();
      eng._events.add({'ev': 'exit', 'code': code});
    });
    await ready.future;
    return eng;
  }

  void _listen(Completer<void> ready) {
    _proc.stderr.drain<void>();
    _proc.stdout.transform(utf8.decoder).transform(const LineSplitter()).listen((line) {
      final Map<String, dynamic> m;
      try {
        m = jsonDecode(line) as Map<String, dynamic>;
      } catch (_) {
        return;
      }
      final ev = m['ev'] as String?;
      if (ev != null) {
        if (ev == 'ready' && !ready.isCompleted) ready.complete();
        if (ev == 'fatal' && !ready.isCompleted) {
          ready.completeError(EngineError(m['error']?.toString() ?? t('err.open')));
        }
        _events.add(m);
        return;
      }
      final c = _pending.remove(m['id']);
      if (c == null) return;
      if (m['ok'] == true) {
        c.complete(m['data']);
      } else {
        c.completeError(EngineError(m['error']?.toString() ?? t('err.generic')));
      }
    });
  }

  Future<dynamic> call(String cmd, [Map<String, dynamic> args = const {}]) {
    if (_closed) return Future.error(EngineError(t('err.closed')));
    final id = _next++;
    final c = Completer<dynamic>();
    _pending[id] = c;
    _proc.stdin.writeln(jsonEncode({'id': id, 'cmd': cmd, ...args}));
    return c.future;
  }

  /// Polkit rules apply only to the installed path. A build running elsewhere
  /// falls back to the generic action when calling pkexec and opens a password window.
  static bool get installed => binary == '/opt/salvage/salvage-engine';

  /// Separate permission for SMART that needs no password. No window opens without it.
  static Future<Map<String, dynamic>?> smart(String dev) async {
    if (!installed) return null;
    try {
      final ok = await Process.run('pkcheck', ['--action-id', 'dev.salvage.Salvage.smart', '--process', '$pid']);
      if (ok.exitCode != 0) return null;
      final r = await Process.run('pkexec', [binary, 'smart', dev], stdoutEncoding: utf8);
      if (r.exitCode != 0) return null;
      return jsonDecode(r.stdout as String) as Map<String, dynamic>;
    } catch (_) {
      return null;
    }
  }

  /// Will scans run without a password prompt?
  static Future<bool> passwordless() async {
    if (!installed) return false;
    try {
      final r = await Process.run('pkcheck', ['--action-id', 'dev.salvage.Salvage.engine', '--process', '$pid']);
      return r.exitCode == 0;
    } catch (_) {
      return false;
    }
  }

  static Future<bool> setPasswordless(bool on) async {
    if (!installed) return false;
    final r = await Process.run('pkexec', [binary, on ? 'allow-user' : 'deny-user']);
    return r.exitCode == 0;
  }

  void close() {
    if (_closed) return;
    _closed = true;
    try {
      _proc.stdin.close();
    } catch (_) {}
  }
}
