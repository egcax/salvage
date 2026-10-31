import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';

import 'engine.dart';
import 'format.dart';
import 'i18n.dart';
import 'models.dart';

String get _home => Platform.environment['HOME'] ?? '/tmp';
String get dataDir => '${Platform.environment['XDG_DATA_HOME'] ?? '$_home/.local/share'}/salvage';
String get configDir => '${Platform.environment['XDG_CONFIG_HOME'] ?? '$_home/.config'}/salvage';

class Settings extends ChangeNotifier {
  ThemeMode themeMode = ThemeMode.dark;
  bool wallpaperAccent = true;
  Color seed = const Color(0xFF7C5CFF);
  Color? wallpaperSeed;
  String defaultDest = '';
  bool autoSave = true;
  bool verifyHash = true;
  bool reduceMotion = false;
  bool notify = true;
  int recoveredFiles = 0;
  int recoveredBytes = 0;
  String language = 'system';
  StreamSubscription<FileSystemEvent>? _watch;

  File get _file => File('$configDir/settings.json');
  File get _colors => File('$_home/.config/quickshell/colors.json');

  Color get activeSeed => wallpaperAccent ? (wallpaperSeed ?? seed) : seed;
  bool get hasWallpaperTheme => wallpaperSeed != null;

  Future<void> load() async {
    try {
      final m = jsonDecode(await _file.readAsString()) as Map<String, dynamic>;
      themeMode = ThemeMode.values.firstWhere((e) => e.name == m['theme'], orElse: () => ThemeMode.dark);
      wallpaperAccent = m['wallpaper'] != false;
      seed = Color((m['seed'] as num?)?.toInt() ?? seed.toARGB32());
      defaultDest = m['dest'] as String? ?? '';
      autoSave = m['autosave'] != false;
      verifyHash = m['verify'] != false;
      reduceMotion = m['reduce_motion'] == true;
      notify = m['notify'] != false;
      recoveredFiles = (m['recovered_files'] as num?)?.toInt() ?? 0;
      recoveredBytes = (m['recovered_bytes'] as num?)?.toInt() ?? 0;
      language = m['language'] as String? ?? 'system';
    } catch (_) {}
    await L.load(language);
    await _readWallpaper();
    // When the wallpaper changes the accent color changes too; event based, no polling.
    if (_colors.parent.existsSync()) {
      _watch = _colors.parent.watch().listen((e) {
        if (e.path.endsWith('colors.json')) _readWallpaper();
      });
    }
    notifyListeners();
  }

  Future<void> _readWallpaper() async {
    try {
      final m = jsonDecode(await _colors.readAsString()) as Map<String, dynamic>;
      final hex = (m['mauve'] as String).replaceFirst('#', '');
      final c = Color(int.parse('FF$hex', radix: 16));
      if (c != wallpaperSeed) {
        wallpaperSeed = c;
        notifyListeners();
      }
    } catch (_) {}
  }

  void update(void Function(Settings s) f) {
    f(this);
    notifyListeners();
    unawaited(_save());
  }

  Future<void> setLanguage(String code) async {
    language = code;
    await L.load(code);
    notifyListeners();
    unawaited(_save());
  }

  Future<void> _save() async {
    await Directory(configDir).create(recursive: true);
    await _file.writeAsString(jsonEncode({
      'theme': themeMode.name,
      'wallpaper': wallpaperAccent,
      'seed': seed.toARGB32(),
      'dest': defaultDest,
      'autosave': autoSave,
      'verify': verifyHash,
      'reduce_motion': reduceMotion,
      'notify': notify,
      'recovered_files': recoveredFiles,
      'recovered_bytes': recoveredBytes,
      'language': language,
    }));
  }

  @override
  void dispose() {
    _watch?.cancel();
    super.dispose();
  }
}

class DeviceStore extends ChangeNotifier {
  List<DeviceNode> devices = [];
  bool loading = true;
  String? error;
  bool fstrimTimer = false;
  final health = <String, bool>{};

  /// snapper configurations: is snapshot protection in place?
  List<String> get snapshotConfigs {
    try {
      return Directory('/etc/snapper/configs').listSync().map((e) => e.path.split('/').last).toList();
    } catch (_) {
      return const [];
    }
  }

  Process? _monitor;
  Timer? _debounce;

  Future<void> start() async {
    await refresh();
    // A udev event reports plugged and removed devices; no polling.
    try {
      _monitor = await Process.start('udevadm', ['monitor', '--udev', '--subsystem-match=block']);
      _monitor!.stderr.drain<void>();
      _monitor!.stdout.listen((_) {
        _debounce?.cancel();
        _debounce = Timer(const Duration(milliseconds: 500), refresh);
      });
    } catch (_) {}
  }

  Future<void> refresh() async {
    try {
      final m = await Engine.devices();
      devices = ((m['devices'] as List?) ?? []).map((e) => DeviceNode(e as Map<String, dynamic>)).toList()
        ..sort((a, b) {
          final r = (b.removable ? 1 : 0) - (a.removable ? 1 : 0);
          return r != 0 ? r : a.name.compareTo(b.name);
        });
      fstrimTimer = m['fstrim_timer'] == true;
      error = null;
      unawaited(_loadHealth());
    } catch (e) {
      error = e.toString();
    }
    loading = false;
    notifyListeners();
  }

  /// Health mark on a card; read only with the passwordless permission.
  Future<void> _loadHealth() async {
    for (final d in devices.toList()) {
      if (health.containsKey(d.path) || d.type == 'loop') continue;
      final s = await Engine.smart(d.path);
      final passed = (s?['smart_status'] as Map?)?['passed'] as bool?;
      if (passed != null) {
        health[d.path] = passed;
        notifyListeners();
      }
    }
  }

  /// Unlocks LUKS; the passphrase goes through stdin, not visible on the command line.
  Future<String?> unlock(String path, String pass) async {
    final p = await Process.start('udisksctl', ['unlock', '-b', path, '--key-file', '/dev/stdin']);
    p.stdin.add(utf8.encode(pass));
    await p.stdin.close();
    final err = await p.stderr.transform(utf8.decoder).join();
    final code = await p.exitCode;
    await refresh();
    return code == 0 ? null : err.trim();
  }

  /// Unmounts a partition with udisks; asks no password in a user session.
  Future<String?> unmount(String path) async {
    final r = await Process.run('udisksctl', ['unmount', '-b', path]);
    await refresh();
    return r.exitCode == 0 ? null : (r.stderr as String).trim();
  }

  @override
  void dispose() {
    _monitor?.kill();
    _debounce?.cancel();
    super.dispose();
  }
}

class SessionRecord {
  final String path, source, title, mode;
  final int created, entries, bytes;
  SessionRecord(this.path, this.source, this.title, this.mode, this.created, this.entries, this.bytes);

  Map<String, dynamic> toJson() => {
        'source': source,
        'title': title,
        'mode': mode,
        'created': created,
        'entries': entries,
        'bytes': bytes,
      };
}

class HistoryStore extends ChangeNotifier {
  List<SessionRecord> items = [];
  Directory get dir => Directory('$dataDir/sessions');

  Future<void> load() async {
    final out = <SessionRecord>[];
    if (await dir.exists()) {
      await for (final f in dir.list()) {
        if (!f.path.endsWith('.json')) continue;
        try {
          final m = jsonDecode(await File(f.path).readAsString()) as Map<String, dynamic>;
          final data = f.path.replaceFirst(RegExp(r'\.json$'), '.salvage');
          if (!File(data).existsSync()) continue;
          out.add(SessionRecord(data, m['source'] as String, m['title'] as String, m['mode'] as String,
              m['created'] as int, m['entries'] as int, m['bytes'] as int));
        } catch (_) {}
      }
    }
    out.sort((a, b) => b.created.compareTo(a.created));
    items = out;
    notifyListeners();
  }

  Future<void> remove(SessionRecord r) async {
    try {
      await File(r.path).delete();
      await File(r.path.replaceFirst(RegExp(r'\.salvage$'), '.json')).delete();
    } catch (_) {}
    await load();
  }
}

enum ScanPhase { connecting, scanning, paused, done, cancelled, failed, loaded }

class ScanSession extends ChangeNotifier {
  final String source;
  final String title;
  final bool elevated;
  final String? sessionFile;
  Engine? engine;

  ScanPhase phase = ScanPhase.connecting;
  String? error;
  String phaseName = 'phase.connect';
  int done = 0, total = 0, found = 0, foundBytes = 0, errors = 0;
  double rate = 0, eta = -1, elapsed = 0;
  String map = '';
  List<VolumeInfo> volumes = [];
  final logs = <String>[];
  Map<String, dynamic>? info;
  Map<String, dynamic>? scanOpts;
  int resultsEpoch = 0;
  StreamSubscription<Map<String, dynamic>>? _sub;
  final _taskEvents = StreamController<Map<String, dynamic>>.broadcast();
  Stream<Map<String, dynamic>> get taskEvents => _taskEvents.stream;
  void Function(ScanSession s)? onFinished;
  void Function(int files, int bytes)? onRecovered;

  ScanSession({required this.source, required this.title, required this.elevated, this.sessionFile});

  bool resumable = false;
  bool holdTrim = false;
  List<String> trimHeld = [];
  int rescued = 0;
  String? rescuedDest;

  bool get _isRescue => phase == ScanPhase.scanning && (scanOpts ?? const {})['rescue_dest'] != null && rescuedDest == null;

  bool get running => phase == ScanPhase.scanning || phase == ScanPhase.paused;
  double? get fraction => total > 0 ? (done / total).clamp(0.0, 1.0) : null;

  Future<void> connect() async {
    try {
      engine = await Engine.open(source: sessionFile == null ? source : null, session: sessionFile, elevate: elevated, holdTrim: holdTrim);
      _sub = engine!.events.listen(_onEvent);
      info = await engine!.call('info') as Map<String, dynamic>?;
      trimHeld = ((info?['trim_held'] as List?) ?? []).cast<String>();
      resumable = ((info?['pending'] as num?) ?? 0) > 0;
      volumes = ((info?['volumes'] as List?) ?? []).map((e) => VolumeInfo(e as Map<String, dynamic>)).toList();
      if (sessionFile != null) {
        phase = ScanPhase.loaded;
        phaseName = 'phase.saved';
        resultsEpoch++;
      }
    } catch (e) {
      phase = ScanPhase.failed;
      error = e.toString();
    }
    notifyListeners();
  }

  Future<void> scan(Map<String, dynamic> opts) async {
    scanOpts = opts;
    done = total = found = foundBytes = errors = 0;
    rate = 0;
    eta = -1;
    map = '';
    phase = ScanPhase.scanning;
    phaseName = 'phase.start';
    resultsEpoch++;
    notifyListeners();
    try {
      await engine!.call('scan', {'opts': opts});
    } catch (e) {
      phase = ScanPhase.failed;
      error = e.toString();
      notifyListeners();
    }
  }

  void _onEvent(Map<String, dynamic> m) {
    switch (m['ev']) {
      case 'progress':
        phaseName = m['phase'] as String? ?? phaseName;
        done = (m['done'] as num?)?.toInt() ?? 0;
        total = (m['total'] as num?)?.toInt() ?? 0;
        found = (m['found'] as num?)?.toInt() ?? 0;
        foundBytes = (m['found_bytes'] as num?)?.toInt() ?? 0;
        rate = (m['rate'] as num?)?.toDouble() ?? 0;
        eta = (m['eta'] as num?)?.toDouble() ?? -1;
        elapsed = (m['elapsed'] as num?)?.toDouble() ?? 0;
        errors = (m['errors'] as num?)?.toInt() ?? 0;
        final mp = m['map'] as String?;
        if (mp != null) map = mp;
        notifyListeners();
      case 'log':
        logs.add(m['msg'] as String? ?? '');
        if (logs.length > 200) logs.removeAt(0);
      case 'scan_done':
        final cancelled = m['cancelled'] == true;
        phase = m['error'] != null ? ScanPhase.failed : (cancelled ? ScanPhase.cancelled : ScanPhase.done);
        error = m['error'] as String?;
        found = (m['found'] as num?)?.toInt() ?? found;
        foundBytes = (m['bytes'] as num?)?.toInt() ?? foundBytes;
        elapsed = (m['seconds'] as num?)?.toDouble() ?? elapsed;
        resumable = m['resumable'] == true;
        phaseName = cancelled ? 'phase.stopped' : 'phase.done';
        volumes = ((m['volumes'] as List?) ?? []).map((e) => VolumeInfo(e as Map<String, dynamic>)).toList();
        resultsEpoch++;
        notifyListeners();
        onFinished?.call(this);
      case 'recover_done' when _isRescue:
        rescued = (m['ok'] as num?)?.toInt() ?? 0;
        rescuedDest = m['dest'] as String?;
        notifyListeners();
      case 'recover' || 'recover_done' || 'image' || 'image_done' || 'task' || 'task_done':
        if (m['ev'] == 'recover_done') onRecovered?.call((m['ok'] as num?)?.toInt() ?? 0, (m['bytes'] as num?)?.toInt() ?? 0);
        _taskEvents.add(m);
      case 'exit':
        if (running) {
          phase = ScanPhase.failed;
          error = t('err.engine', {'c': m['code']});
          notifyListeners();
        }
    }
  }

  Future<void> pause(bool on) async {
    await engine?.call(on ? 'pause' : 'resume');
    phase = on ? ScanPhase.paused : ScanPhase.scanning;
    notifyListeners();
  }

  Future<void> cancelScan() async => engine?.call('cancel');
  Future<void> cancelTask() async => engine?.call('cancel_task');

  @override
  void dispose() {
    _sub?.cancel();
    _taskEvents.close();
    engine?.close();
    super.dispose();
  }
}

/// Result list: filter, sort, paged cache, selection.
class Results extends ChangeNotifier {
  static const pageSize = 200;
  final ScanSession session;
  Results(this.session) {
    session.addListener(_onSession);
    _epoch = session.resultsEpoch;
    _refreshTimer = Timer.periodic(const Duration(seconds: 2), (_) {
      if (session.running && session.found != _lastFound) {
        _lastFound = session.found;
        refresh();
      }
    });
    refresh();
  }

  // Filter state
  Set<FileState> states = {FileState.deleted, FileState.trash, FileState.carved, FileState.orphan};
  Set<FileKind> kinds = {};
  String text = '';
  String sortBy = 'smart';
  bool sortDesc = true;
  int? dirId;
  String? dirName;
  bool onlyRecoverable = false;
  bool hideDupes = false;
  bool grepOnly = false;
  int? minSize, maxSize;
  DateTimeRange? dateRange;
  bool grid = false;

  // Data
  int totalCount = 0;
  int totalBytes = 0;
  Map<String, List<int>> kindFacets = {};
  Map<String, List<int>> stateFacets = {};
  List<List<int>> chances = [];
  final _pages = <int, List<FileItem>>{};
  final _loading = <int>{};
  int _gen = 0;
  bool firstLoad = true;
  bool _autoSnapshot = false;

  // Selection
  final selected = <int>{};
  final _selectedBytes = <int, int>{};
  bool allMatching = false;
  final except = <int>{};
  /// Preview focus: changing it does not rebuild the list.
  final focusN = ValueNotifier<FileItem?>(null);
  FileItem? get focused => focusN.value;

  int _epoch = 0;
  int _lastFound = -1;
  Timer? _refreshTimer;

  void _onSession() {
    if (session.resultsEpoch != _epoch) {
      _epoch = session.resultsEpoch;
      refresh();
    }
  }

  Map<String, dynamic> get filter => {
        'states': states.map((e) => e.key).toList(),
        'kinds': kinds.map((e) => e.key).toList(),
        'text': text,
        if (dirId != null) 'dir': dirId,
        'recursive': true,
        if (onlyRecoverable) 'min_chance': 10,
        'hide_dupes': hideDupes,
        'grep_only': grepOnly,
        if (minSize != null) 'min_size': minSize,
        if (maxSize != null) 'max_size': maxSize,
        if (dateRange != null) 'from': dateRange!.start.millisecondsSinceEpoch ~/ 1000,
        if (dateRange != null) 'to': dateRange!.end.add(const Duration(days: 1)).millisecondsSinceEpoch ~/ 1000,
      };

  Map<String, dynamic> get sort => {'by': sortBy, 'desc': sortDesc};

  Future<void> refresh({bool reset = false}) async {
    final e = session.engine;
    if (e == null) return;
    final gen = ++_gen;
    try {
      final r = await e.call('query', {'filter': filter, 'sort': sort, 'offset': 0, 'limit': pageSize}) as Map<String, dynamic>;
      if (gen != _gen) return;
      totalCount = (r['total'] as num).toInt();
      totalBytes = (r['bytes'] as num?)?.toInt() ?? 0;
      final fx = r['facets'] as Map<String, dynamic>;
      kindFacets = _facet(fx['kinds']);
      stateFacets = _facet(fx['states']);
      chances = (fx['chances'] as List).map((e) => (e as List).map((x) => (x as num).toInt()).toList()).toList();
      _pages
        ..clear()
        ..[0] = (r['items'] as List).map((j) => FileItem.fromJson(j as Map<String, dynamic>)).toList();
      _loading.clear();
      firstLoad = false;
      // With no deleted records but files in a snapshot, do not leave the user on an empty screen.
      if (!_autoSnapshot && !session.running && totalCount == 0 && (stateFacets['snapshot']?[0] ?? 0) > 0) {
        _autoSnapshot = true;
        states = {...states, FileState.snapshot};
        await refresh();
        return;
      }
      notifyListeners();
    } catch (_) {}
  }

  Map<String, List<int>> _facet(dynamic m) => (m as Map<String, dynamic>)
      .map((k, v) => MapEntry(k, (v as List).map((e) => (e as num).toInt()).toList()));

  FileItem? itemAt(int i) {
    final p = i ~/ pageSize;
    final page = _pages[p];
    if (page == null) {
      _loadPage(p);
      return null;
    }
    final k = i - p * pageSize;
    return k < page.length ? page[k] : null;
  }

  Future<void> _loadPage(int p) async {
    if (!_loading.add(p)) return;
    final gen = _gen;
    try {
      final r = await session.engine!.call('query', {
        'filter': filter,
        'sort': sort,
        'offset': p * pageSize,
        'limit': pageSize,
      }) as Map<String, dynamic>;
      if (gen != _gen) return;
      _pages[p] = (r['items'] as List).map((j) => FileItem.fromJson(j as Map<String, dynamic>)).toList();
      notifyListeners();
    } catch (_) {
    } finally {
      _loading.remove(p);
    }
  }

  void change(void Function(Results r) f) {
    f(this);
    allMatching = false;
    except.clear();
    selected.clear();
    _selectedBytes.clear();
    refresh();
    notifyListeners();
  }

  void setGrid(bool v) {
    grid = v;
    notifyListeners();
  }

  void focus(FileItem? i) => focusN.value = i;

  bool isSelected(FileItem i) => allMatching ? !except.contains(i.id) : selected.contains(i.id);

  void toggle(FileItem i) {
    if (allMatching) {
      except.contains(i.id) ? except.remove(i.id) : except.add(i.id);
    } else if (selected.remove(i.id)) {
      _selectedBytes.remove(i.id);
    } else {
      selected.add(i.id);
      _selectedBytes[i.id] = i.size;
    }
    notifyListeners();
  }

  /// Everything recoverable: items counted as lost stay out.
  void selectRecoverable() {
    change((x) => x.onlyRecoverable = true);
    allMatching = true;
    except.clear();
    notifyListeners();
  }

  void selectAll() {
    allMatching = true;
    except.clear();
    notifyListeners();
  }

  void clearSelection() {
    allMatching = false;
    except.clear();
    selected.clear();
    _selectedBytes.clear();
    notifyListeners();
  }

  int get selectionCount => allMatching ? totalCount - except.length : selected.length;
  int get selectionBytes => allMatching ? totalBytes : _selectedBytes.values.fold(0, (a, b) => a + b);

  Map<String, dynamic> get selection => allMatching
      ? {'filter': filter, 'except': except.toList()}
      : {'ids': selected.toList()};

  @override
  void dispose() {
    session.removeListener(_onSession);
    _refreshTimer?.cancel();
    focusN.dispose();
    super.dispose();
  }
}

/// Thumbnail queue: requests that scroll out are cancelled, the newest comes first.
class ThumbLoader {
  final Engine engine;
  ThumbLoader(this.engine);
  final _memo = <String, String?>{};
  final _queue = <_ThumbReq>[];
  int _active = 0;

  Future<String?> get(int id, int size) {
    final key = '$id:$size';
    if (_memo.containsKey(key)) return Future.value(_memo[key]);
    final r = _ThumbReq(id, size);
    _queue.add(r);
    _pump();
    return r.c.future;
  }

  void cancel(int id, int size) {
    _queue.removeWhere((r) {
      final hit = r.id == id && r.size == size;
      if (hit) r.c.complete(null);
      return hit;
    });
  }

  void _pump() {
    while (_active < 2 && _queue.isNotEmpty) {
      final r = _queue.removeLast();
      _active++;
      engine.call('thumb', {'entry': r.id, 'size': r.size}).then((v) {
        final p = (v as Map<String, dynamic>)['path'] as String?;
        _memo['${r.id}:${r.size}'] = p;
        if (_memo.length > 4000) _memo.remove(_memo.keys.first);
        r.c.complete(p);
      }).catchError((Object _) {
        r.c.complete(null);
      }).whenComplete(() {
        _active--;
        _pump();
      });
    }
  }
}

class _ThumbReq {
  final int id, size;
  final c = Completer<String?>();
  _ThumbReq(this.id, this.size);
}

/// App-wide state.
class AppModel extends ChangeNotifier {
  final settings = Settings();
  final devices = DeviceStore();
  final history = HistoryStore();
  ScanSession? session;
  Results? results;
  ThumbLoader? thumbs;
  int page = 0; // 0 sources, 1 scan, 2 history, 3 settings

  Future<void> initRest() async {
    await history.load();
    await devices.start();
  }

  void go(int p) {
    page = p;
    notifyListeners();
  }

  Process? _inhibit;

  /// Keep the system from sleeping during a long scan.
  Future<void> _holdAwake(bool on) async {
    _inhibit?.kill();
    _inhibit = null;
    if (!on) return;
    try {
      _inhibit = await Process.start('systemd-inhibit', ['--what=sleep:idle', '--who=Salvage', '--why=Scanning', 'sleep', 'infinity']);
    } catch (_) {}
  }

  void _notifyDone(ScanSession s) {
    if (!settings.notify || s.phase != ScanPhase.done) return;
    Process.run('notify-send', ['-a', 'Salvage', '-i', 'dev.salvage.Salvage', t('notif.title'), t('notif.body', {'n': fmtCount(s.found), 'b': fmtBytes(s.foundBytes)})]).catchError((Object _) => ProcessResult(0, 0, '', ''));
  }

  Future<void> start({required String source, required String title, required Map<String, dynamic> opts, bool holdTrim = false}) async {
    closeSession();
    final s = ScanSession(source: source, title: title, elevated: source.startsWith('/dev/'));
    s.holdTrim = holdTrim;
    s.onFinished = (x) {
      _holdAwake(false);
      _notifyDone(x);
      _autosave(x);
    };
    _attach(s);
    await s.connect();
    if (s.phase == ScanPhase.failed) {
      notifyListeners();
      return;
    }
    thumbs = ThumbLoader(s.engine!);
    results = Results(s);
    notifyListeners();
    unawaited(_holdAwake(true));
    await s.scan(opts);
  }

  /// Resumes an interrupted scan from a saved session.
  Future<void> resume() async {
    final s = session;
    if (s == null) return;
    s.onFinished = (x) {
      _holdAwake(false);
      _notifyDone(x);
      _autosave(x);
    };
    unawaited(_holdAwake(true));
    await s.scan({'mode': 'full', 'resume': true, 'step': 512});
  }

  Future<void> reopen(SessionRecord r) async {
    closeSession();
    final s = ScanSession(source: r.source, title: r.title, elevated: r.source.startsWith('/dev/'), sessionFile: r.path);
    _attach(s);
    await s.connect();
    if (s.phase != ScanPhase.failed) {
      thumbs = ThumbLoader(s.engine!);
      results = Results(s);
      s.found = r.entries;
      s.foundBytes = r.bytes;
    }
    notifyListeners();
  }

  void _attach(ScanSession s) {
    s.onRecovered = (files, bytes) => settings.update((x) {
          x.recoveredFiles += files;
          x.recoveredBytes += bytes;
        });
    session = s;
    page = 1;
    notifyListeners();
  }

  void closeSession() {
    _holdAwake(false);
    final (r, s) = (results, session);
    results = null;
    thumbs = null;
    session = null;
    if (page == 1) page = 0;
    notifyListeners();
    // Release once the listening screens have left the tree.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      r?.dispose();
      s?.dispose();
    });
  }

  Future<void> _autosave(ScanSession s) async {
    final keep = s.phase == ScanPhase.done || (s.phase == ScanPhase.cancelled && s.resumable);
    if (!settings.autoSave || s.found == 0 || !keep) return;
    try {
      // A saved session on the scanned partition could overwrite data to be recovered.
      final check = await s.engine!.call('check_dest', {'path': Platform.environment['HOME'] ?? '/'}) as Map<String, dynamic>;
      if (check['same_partition'] == true) return;
      await history.dir.create(recursive: true);
      final ts = DateTime.now().millisecondsSinceEpoch;
      final path = '${history.dir.path}/$ts.salvage';
      await s.engine!.call('save_session', {'path': path});
      final rec = SessionRecord(path, s.source, s.title, (s.scanOpts?['mode'] as String?) ?? 'deep', ts ~/ 1000, s.found, s.foundBytes);
      await File('${history.dir.path}/$ts.json').writeAsString(jsonEncode(rec.toJson()));
      await history.load();
    } catch (_) {}
  }

  @override
  void dispose() {
    closeSession();
    settings.dispose();
    devices.dispose();
    history.dispose();
    super.dispose();
  }
}

class AppScope extends InheritedNotifier<AppModel> {
  const AppScope({super.key, required AppModel model, required super.child}) : super(notifier: model);
  static AppModel of(BuildContext c) => c.dependOnInheritedWidgetOfExactType<AppScope>()!.notifier!;
  static AppModel read(BuildContext c) => c.findAncestorWidgetOfExactType<AppScope>()!.notifier!;
}
