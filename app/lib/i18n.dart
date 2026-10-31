import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:intl/date_symbol_data_local.dart';

class Lang {
  final String code, name;
  final Locale locale;
  const Lang(this.code, this.name, this.locale);
}

const languages = [
  Lang('en', 'English', Locale('en')),
  Lang('tr', 'Türkçe', Locale('tr')),
  Lang('de', 'Deutsch', Locale('de')),
  Lang('fr', 'Français', Locale('fr')),
  Lang('es', 'Español', Locale('es')),
  Lang('pt', 'Português', Locale('pt')),
  Lang('it', 'Italiano', Locale('it')),
  Lang('nl', 'Nederlands', Locale('nl')),
  Lang('pl', 'Polski', Locale('pl')),
  Lang('sv', 'Svenska', Locale('sv')),
  Lang('cs', 'Čeština', Locale('cs')),
  Lang('ro', 'Română', Locale('ro')),
  Lang('el', 'Ελληνικά', Locale('el')),
  Lang('ru', 'Русский', Locale('ru')),
  Lang('uk', 'Українська', Locale('uk')),
  Lang('ar', 'العربية', Locale('ar')),
  Lang('hi', 'हिन्दी', Locale('hi')),
  Lang('id', 'Bahasa Indonesia', Locale('id')),
  Lang('vi', 'Tiếng Việt', Locale('vi')),
  Lang('th', 'ไทย', Locale('th')),
  Lang('ja', '日本語', Locale('ja')),
  Lang('ko', '한국어', Locale('ko')),
  Lang('zh', '简体中文', Locale('zh')),
  Lang('zh_TW', '繁體中文', Locale('zh', 'TW')),
];

class L {
  static Map<String, String> _m = {};
  static Map<String, String> _en = {};
  static Lang current = languages.first;
  static String get tag => current.code;

  static Lang systemLang() {
    final raw = Platform.environment['LC_ALL'] ?? Platform.environment['LC_MESSAGES'] ?? Platform.environment['LANG'] ?? Platform.localeName;
    final s = raw.split('.').first.replaceAll('-', '_');
    final exact = languages.where((l) => l.code == s);
    if (exact.isNotEmpty) return exact.first;
    if (s == 'zh_HK' || s == 'zh_MO') return languages.firstWhere((l) => l.code == 'zh_TW');
    final base = s.split('_').first;
    return languages.firstWhere((l) => l.code == base, orElse: () => languages.first);
  }

  static Future<Map<String, String>> _read(String code) async {
    final raw = await rootBundle.loadString('assets/i18n/$code.json');
    return (jsonDecode(raw) as Map<String, dynamic>).cast<String, String>();
  }

  /// `choice`: 'system' or a language code.
  static Future<void> load(String choice) async {
    current = choice == 'system' ? systemLang() : languages.firstWhere((l) => l.code == choice, orElse: systemLang);
    if (_en.isEmpty) _en = await _read('en');
    try {
      _m = current.code == 'en' ? _en : await _read(current.code);
    } catch (_) {
      _m = _en;
    }
    await initializeDateFormatting(current.code);
  }
}

String t(String key, [Map<String, Object?>? args]) {
  var s = L._m[key] ?? L._en[key] ?? key;
  if (args != null) {
    args.forEach((k, v) => s = s.replaceAll('{$k}', '$v'));
  }
  return s;
}

/// Engine messages in the form "code|argument".
String tn(String? code) {
  if (code == null) return '';
  final p = code.split('|');
  return t(p.first, {if (p.length > 1) 'a': p[1]});
}

/// English fixed folder names from the engine.
String dirLabel(String name) => switch (name) {
      r'$Orphans' => t('dir.orphans'),
      r'$Carved' => t('dir.carved'),
      r'$Recycle Bin' => t('dir.recycle'),
      'Images' => t('kind.image'),
      'Videos' => t('kind.video'),
      'Audio' => t('kind.audio'),
      'Documents' => t('kind.document'),
      'Archives' => t('kind.archive'),
      'Text and code' => t('kind.code'),
      'Applications' => t('kind.app'),
      'Databases' => t('kind.database'),
      'Other' => t('kind.other'),
      _ => name,
    };

String pathLabel(String path) => path.split('/').map(dirLabel).join('/');
