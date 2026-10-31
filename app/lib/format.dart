import 'package:intl/intl.dart';

import 'i18n.dart';

NumberFormat? _int;
NumberFormat? _dec;
String? _tag;

void _ensure() {
  if (_tag == L.tag) return;
  _tag = L.tag;
  _int = NumberFormat.decimalPattern(L.tag);
  _dec = NumberFormat('0.0', L.tag);
}

String fmtBytes(num b) {
  _ensure();
  if (b < 1024) return '${b.toInt()} B';
  const u = ['KB', 'MB', 'GB', 'TB', 'PB'];
  var v = b / 1024;
  var i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return '${v >= 100 ? _int!.format(v.round()) : _dec!.format(v)} ${u[i]}';
}

String fmtCount(num n) {
  _ensure();
  return _int!.format(n.toInt());
}

String fmtDate(int unix, {bool time = true}) {
  if (unix <= 0) return '—';
  final d = DateTime.fromMillisecondsSinceEpoch(unix * 1000);
  final f = time ? DateFormat.yMMMd(L.tag).add_Hm() : DateFormat.yMMMd(L.tag);
  return f.format(d);
}

String fmtDuration(double s) {
  if (s < 0 || s.isNaN || s.isInfinite) return '—';
  if (s < 60) return t('time.s', {'s': s.ceil()});
  if (s < 3600) return t('time.ms', {'m': (s / 60).floor(), 's': (s % 60).floor()});
  return t('time.hm', {'h': (s / 3600).floor(), 'm': ((s % 3600) / 60).floor()});
}

String fmtRate(double r) => '${fmtBytes(r)}/s';
