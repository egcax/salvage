import 'package:flutter/material.dart';

import 'i18n.dart';

class DeviceNode {
  final Map<String, dynamic> raw;
  final List<DeviceNode> children;
  DeviceNode(this.raw) : children = ((raw['children'] as List?) ?? []).map((e) => DeviceNode(e as Map<String, dynamic>)).toList();

  String get path => raw['path'] as String? ?? '';
  String get name => raw['name'] as String? ?? '';
  String get type => raw['type'] as String? ?? '';
  int get size => (raw['size'] as num?)?.toInt() ?? 0;
  String? get fstype => raw['fstype'] as String?;
  String? get label => (raw['label'] as String?)?.trim().nullIfEmpty ?? (raw['partlabel'] as String?)?.trim().nullIfEmpty;
  String? get transport => raw['tran'] as String?;
  bool get removable => raw['rm'] == true || raw['hotplug'] == true;
  bool get rotational => raw['rota'] == true;
  bool get trim => raw['trim'] == true;
  bool get discardMount => raw['discard_mount'] == true;
  String? get mountedAt => raw['mounted_at'] as String?;
  int? get fsTotal => (raw['fs_total'] as num?)?.toInt();
  int? get fsFree => (raw['fs_free'] as num?)?.toInt();
  bool get locked => fstype == 'crypto_LUKS' && children.isEmpty;

  /// LUKS and LVM volumes are nested under a partition.
  List<DeviceNode> get flat => [for (final c in children) ...[c, ...c.flat]];

  List<String> get mountpoints => ((raw['mountpoints'] as List?) ?? []).whereType<String>().toList();
  bool get mounted => mountpoints.isNotEmpty;
  bool get anyMounted => mounted || flat.any((c) => c.mounted);
  bool get anyDiscard => discardMount || flat.any((c) => c.discardMount);

  String get title {
    final m = ((raw['model'] as String?) ?? (raw['sysmodel'] as String?) ?? '').trim();
    final v = ((raw['vendor'] as String?) ?? '').trim();
    final s = [v, m].where((e) => e.isNotEmpty).join(' ');
    if (s.isNotEmpty) return s;
    return type == 'loop' ? t('dev.image') : name;
  }

  IconData get icon {
    if (type == 'loop') return Icons.album_rounded;
    if (transport == 'usb') return Icons.usb_rounded;
    if (name.startsWith('mmc')) return Icons.sd_card_rounded;
    if (transport == 'nvme') return Icons.memory_rounded;
    return Icons.storage_rounded;
  }

  IconData get partIcon {
    if (locked) return Icons.lock_rounded;
    if (type == 'crypt') return Icons.lock_open_rounded;
    if (type == 'lvm') return Icons.layers_rounded;
    return mounted ? Icons.folder_open_rounded : Icons.folder_rounded;
  }

  String get transportLabel => switch (transport) {
        'usb' => 'USB',
        'nvme' => 'NVMe SSD',
        'sata' => rotational ? 'SATA HDD' : 'SATA SSD',
        'mmc' => t('dev.sd'),
        _ => type == 'loop' ? t('dev.loop') : t('dev.disk'),
      };
}

extension NullIfEmpty on String {
  String? get nullIfEmpty => isEmpty ? null : this;
}

enum FileKind {
  image('image', Icons.image_rounded),
  video('video', Icons.movie_rounded),
  audio('audio', Icons.music_note_rounded),
  document('document', Icons.description_rounded),
  archive('archive', Icons.folder_zip_rounded),
  code('code', Icons.code_rounded),
  app('app', Icons.apps_rounded),
  database('database', Icons.dataset_rounded),
  other('other', Icons.insert_drive_file_rounded);

  final String key;
  final IconData icon;
  const FileKind(this.key, this.icon);
  String get label => t('kind.$key');
  static FileKind of(String? k) => values.firstWhere((e) => e.key == k, orElse: () => other);
}

enum FileState {
  deleted('deleted', Icons.delete_sweep_rounded),
  trash('trash', Icons.delete_rounded),
  carved('carved', Icons.fingerprint_rounded),
  orphan('orphan', Icons.link_off_rounded),
  snapshot('snapshot', Icons.photo_camera_rounded),
  existing('existing', Icons.check_circle_rounded);

  final String key;
  final IconData icon;
  const FileState(this.key, this.icon);
  String get label => t('state.$key');
  static FileState of(String? k) => values.firstWhere((e) => e.key == k, orElse: () => deleted);
}

class FileItem {
  final int id, size, mtime, ctime, dtime, chance, vol, dir;
  final String name, ext, method, path;
  final String? note;
  final FileKind kind;
  final FileState state;
  final bool dup, verified;

  FileItem.fromJson(Map<String, dynamic> j)
      : id = j['id'] as int,
        size = (j['size'] as num).toInt(),
        mtime = (j['mtime'] as num?)?.toInt() ?? 0,
        ctime = (j['ctime'] as num?)?.toInt() ?? 0,
        dtime = (j['dtime'] as num?)?.toInt() ?? 0,
        chance = (j['chance'] as num?)?.toInt() ?? 0,
        vol = (j['vol'] as num?)?.toInt() ?? 0,
        dir = (j['dir'] as num?)?.toInt() ?? 0,
        name = j['name'] as String? ?? '',
        ext = j['ext'] as String? ?? '',
        method = j['method'] as String? ?? '',
        path = j['path'] as String? ?? '',
        note = j['note'] as String?,
        kind = FileKind.of(j['kind'] as String?),
        state = FileState.of(j['state'] as String?),
        dup = j['dup'] == true,
        verified = j['verified'] == true;

  int get bestDate => mtime > dtime ? mtime : dtime;
  bool get previewAsImage => kind == FileKind.image || kind == FileKind.video || ext == 'pdf';
  bool get previewAsText =>
      kind == FileKind.code || const {'txt', 'md', 'csv', 'log', 'ini', 'conf', 'rtf', 'vcf', 'ics', 'tex'}.contains(ext);
}

class DirNode {
  final int id, count, bytes;
  final String name;
  final bool deleted, hasChildren;
  DirNode.fromJson(Map<String, dynamic> j)
      : id = j['id'] as int,
        count = (j['count'] as num).toInt(),
        bytes = (j['bytes'] as num).toInt(),
        name = j['name'] as String? ?? '',
        deleted = j['deleted'] == true,
        hasChildren = j['has_children'] == true;
}

class VolumeInfo {
  final Map<String, dynamic> raw;
  VolumeInfo(this.raw);
  String get fstype => raw['fstype'] as String? ?? '';
  String get label => raw['label'] as String? ?? '';
  int get size => (raw['size'] as num?)?.toInt() ?? 0;
  int get freeBytes => (raw['free_bytes'] as num?)?.toInt() ?? 0;
  double? get zeroRatio => (raw['zero_ratio'] as num?)?.toDouble();
  bool get lost => raw['lost'] == true;
  String? get note => raw['note'] as String?;
}

class ChanceInfo {
  final String label;
  final int tier;
  const ChanceInfo(this.label, this.tier);
  static ChanceInfo of(int c) {
    if (c >= 80) return ChanceInfo(t('chance.high'), 3);
    if (c >= 40) return ChanceInfo(t('chance.mid'), 2);
    if (c >= 10) return ChanceInfo(t('chance.low'), 1);
    return ChanceInfo(t('chance.lost'), 0);
  }
}
