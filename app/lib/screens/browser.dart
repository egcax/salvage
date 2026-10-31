import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../format.dart';
import '../i18n.dart';
import '../models.dart';
import '../state.dart';
import '../theme.dart';
import '../widgets.dart';
import 'preview_panel.dart';
import 'tools.dart';

class Browser extends StatefulWidget {
  final Results results;
  final ScanSession session;
  const Browser({super.key, required this.results, required this.session});
  @override
  State<Browser> createState() => _BrowserState();
}

class _BrowserState extends State<Browser> {
  final _search = TextEditingController();
  final _searchFocus = FocusNode();
  final _scroll = ScrollController();
  Timer? _debounce;

  Results get r => widget.results;

  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_onKey);
  }

  /// The keyboard is listened to directly so shortcuts work even when focus is lost
  /// after a dialog closes; it responds only when this page is in front with no dialog on top.
  bool _onKey(KeyEvent e) {
    if (e is! KeyDownEvent || !mounted) return false;
    if (AppScope.read(context).page != 1 || !(ModalRoute.of(context)?.isCurrent ?? false)) return false;
    final ctrl = HardwareKeyboard.instance.isControlPressed;
    final typing = _searchFocus.hasFocus;
    final k = e.logicalKey;
    if (k == LogicalKeyboardKey.f1) {
      showShortcuts(context);
    } else if (k == LogicalKeyboardKey.escape) {
      if (typing) {
        _searchFocus.unfocus();
      } else if (r.focused != null) {
        r.focus(null);
      } else {
        r.clearSelection();
      }
    } else if (ctrl && k == LogicalKeyboardKey.keyF) {
      _searchFocus.requestFocus();
    } else if (ctrl && k == LogicalKeyboardKey.keyA && !typing) {
      r.selectAll();
    } else if (ctrl && k == LogicalKeyboardKey.keyR) {
      if (r.selectionCount > 0) showRecover(context, widget.session, r);
    } else if (ctrl && k == LogicalKeyboardKey.keyG) {
      r.setGrid(!r.grid);
    } else {
      return false;
    }
    return true;
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_onKey);
    _search.dispose();
    _searchFocus.dispose();
    _scroll.dispose();
    _debounce?.cancel();
    super.dispose();
  }

  void _onSearch(String v) {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 220), () {
      r.change((x) => x.text = v);
      if (_scroll.hasClients) _scroll.jumpTo(0);
    });
  }

  @override
  Widget build(BuildContext context) {
    return Focus(
      autofocus: true,
      child: Focus(
        canRequestFocus: false,
        child: ListenableBuilder(
          listenable: r,
          builder: (context, _) {
            return LayoutBuilder(builder: (context, c) {
              final wide = c.maxWidth > 1240;
              final inspector = c.maxWidth >= 1000;
              final compact = c.maxWidth - 236 - (inspector ? 320 : 0) < 560;
              return Stack(
                children: [
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      SizedBox(width: 236, child: _Sidebar(r: r)),
                      Expanded(child: _center(compact)),
                      // On a focus change only the panel is rebuilt; the list stays untouched.
                      ValueListenableBuilder<FileItem?>(
                        valueListenable: r.focusN,
                        builder: (_, f, _) => f == null
                            ? (inspector ? SizedBox(width: 320, child: RepaintBoundary(child: _Summary(r: r))) : const SizedBox.shrink())
                            : SizedBox(
                                width: wide ? 400 : 340,
                                child: TweenAnimationBuilder<double>(
                                  key: ValueKey(f.id),
                                  tween: Tween(begin: 1, end: 0),
                                  duration: Motion.medium,
                                  curve: Motion.enter,
                                  builder: (_, v, c) => Transform.translate(offset: Offset(36 * v, 0), child: c),
                                  child: RepaintBoundary(child: PreviewPanel(item: f, results: r)),
                                ),
                              ),
                      ),
                    ],
                  ),
                  Positioned(left: 0, right: 0, bottom: 18, child: _SelectionBar(r: r, session: widget.session)),
                ],
              );
            });
          },
        ),
      ),
    );
  }

  Widget _center(bool compact) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(8, 10, 12, 4),
          child: Row(
            children: [
              Expanded(
                child: SizedBox(
                  height: 46,
                  child: SearchBar(
                    controller: _search,
                    focusNode: _searchFocus,
                    hintText: t('browse.search'),
                    leading: const Padding(padding: EdgeInsets.only(left: 4), child: Icon(Icons.search_rounded)),
                    elevation: const WidgetStatePropertyAll(0),
                    backgroundColor: WidgetStatePropertyAll(Theme.of(context).colorScheme.surfaceContainerHigh),
                    onChanged: _onSearch,
                    trailing: [
                      if (r.grepOnly)
                        InputChip(
                          label: Text(t('browse.in_content')),
                          onDeleted: () => r.change((x) => x.grepOnly = false),
                        ),
                      if (_search.text.isNotEmpty)
                        IconButton(
                          icon: const Icon(Icons.close_rounded),
                          onPressed: () {
                            _search.clear();
                            _onSearch('');
                          },
                        ),
                    ],
                  ),
                ),
              ),
              const SizedBox(width: 8),
              _SortMenu(r: r),
              _FilterButton(r: r),
              _ToolsMenu(r: r, session: widget.session),
              const SizedBox(width: 4),
              SegmentedButton<bool>(
                showSelectedIcon: false,
                segments: [
                  ButtonSegment(value: false, icon: const Icon(Icons.view_list_rounded), tooltip: t('browse.list')),
                  ButtonSegment(value: true, icon: const Icon(Icons.grid_view_rounded), tooltip: t('browse.grid')),
                ],
                selected: {r.grid},
                onSelectionChanged: (v) => r.setGrid(v.first),
              ),
            ],
          ),
        ),
        _StateChips(r: r),
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 2, 12, 4),
          child: Row(
            children: [
              Text(
                t('browse.count', {'n': fmtCount(r.totalCount), 'b': fmtBytes(r.totalBytes)}),
                style: Theme.of(context).textTheme.labelMedium?.copyWith(color: Theme.of(context).colorScheme.onSurfaceVariant),
              ),
              const Spacer(),
              if (r.totalCount > 0)
                TextButton(
                  style: TextButton.styleFrom(visualDensity: VisualDensity.compact),
                  onPressed: r.selectionCount == r.totalCount ? r.clearSelection : r.selectAll,
                  child: Text(r.selectionCount == r.totalCount ? t('browse.unselect') : t('browse.select_all')),
                ),
            ],
          ),
        ),
        if (!r.grid && r.totalCount > 0) _Columns(r: r, compact: compact),
        Expanded(child: _body(compact)),
      ],
    );
  }

  Widget _body(bool compact) {
    if (r.totalCount == 0) {
      if (r.firstLoad || widget.session.running) {
        return EmptyState(icon: Icons.travel_explore_rounded, title: t('browse.scanning'), body: t('browse.scanning_body'));
      }
      return EmptyState(
        icon: Icons.search_off_rounded,
        title: t('browse.none'),
        body: t('browse.none_body'),
        action: OutlinedButton(
          onPressed: () {
            _search.clear();
            r.change((x) {
              x.text = '';
              x.kinds = {};
              x.dirId = null;
              x.dirName = null;
              x.onlyRecoverable = false;
              x.minSize = null;
              x.maxSize = null;
              x.dateRange = null;
              x.states = FileState.values.toSet();
            });
          },
          child: Text(t('browse.clear')),
        ),
      );
    }
    return AnimatedSwitcher(
      duration: Motion.medium,
      switchInCurve: Motion.enter,
      child: r.grid
          ? GridView.builder(
              key: const ValueKey('grid'),
              controller: _scroll,
              padding: const EdgeInsets.fromLTRB(8, 4, 12, 96),
              gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(maxCrossAxisExtent: 172, mainAxisSpacing: 8, crossAxisSpacing: 8, childAspectRatio: 0.82),
              itemCount: r.totalCount,
              itemBuilder: (_, i) {
                final it = r.itemAt(i);
                return it == null ? const _Skeleton(grid: true) : _GridTile(item: it, r: r);
              },
            )
          : ListView.builder(
              key: const ValueKey('list'),
              controller: _scroll,
              padding: const EdgeInsets.fromLTRB(8, 0, 12, 96),
              itemExtent: 64,
              itemCount: r.totalCount,
              itemBuilder: (_, i) {
                final it = r.itemAt(i);
                return it == null ? const _Skeleton() : _Row(item: it, r: r, compact: compact);
              },
            ),
    );
  }
}

/// Sortable column headers; widths match the rows.
class _Columns extends StatelessWidget {
  final Results r;
  final bool compact;
  const _Columns({required this.r, this.compact = false});

  Widget _head(BuildContext context, String key, String label, {required bool end, bool desc = true}) {
    final tx = Theme.of(context).textTheme;
    final cs = Theme.of(context).colorScheme;
    final on = r.sortBy == key;
    final style = tx.labelMedium?.copyWith(color: on ? cs.primary : cs.onSurfaceVariant, fontWeight: on ? FontWeight.w600 : null);
    return InkWell(
      borderRadius: BorderRadius.circular(8),
      onTap: () => r.change((x) {
        x.sortDesc = on ? !x.sortDesc : desc;
        x.sortBy = key;
      }),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 5),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          mainAxisAlignment: end ? MainAxisAlignment.end : MainAxisAlignment.start,
          children: [
            Flexible(child: Text(label, style: style, maxLines: 1, overflow: TextOverflow.ellipsis)),
            if (on) Icon(r.sortDesc ? Icons.arrow_downward_rounded : Icons.arrow_upward_rounded, size: 13, color: cs.primary),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 0, 12, 2),
      child: Row(
        children: [
          const SizedBox(width: 106),
          Expanded(child: Align(alignment: AlignmentDirectional.centerStart, child: _head(context, 'name', t('col.name'), end: false, desc: false))),
          const SizedBox(width: 10),
          SizedBox(width: 78, child: _head(context, 'size', t('col.size'), end: true)),
          if (!compact) ...[
            const SizedBox(width: 14),
            SizedBox(width: 92, child: Align(alignment: AlignmentDirectional.centerStart, child: _head(context, 'date', t('col.date'), end: false))),
          ],
          SizedBox(width: 62, child: _head(context, 'chance', t('col.chance'), end: true)),
          const SizedBox(width: 12),
        ],
      ),
    );
  }
}

/// Right panel with no file selected: the scan at a glance.
class _Summary extends StatelessWidget {
  final Results r;
  const _Summary({required this.r});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final st = Theme.of(context).extension<StatusColors>()!;
    final kc = Theme.of(context).extension<KindColors>()!;
    final c = r.chances;
    final tiers = [(st.onGood, st.good, t('chance.high')), (st.onFair, st.fair, t('chance.mid')), (st.onPoor, st.poor, t('chance.low')), (st.onLost, st.lost, t('chance.lost'))];
    final total = c.fold<int>(0, (a, b) => a + b[0]);
    final kinds = [for (final k in FileKind.values) if ((r.kindFacets[k.key]?[0] ?? 0) > 0) k]..sort((a, b) => r.kindFacets[b.key]![1].compareTo(r.kindFacets[a.key]![1]));
    final maxB = kinds.isEmpty ? 1 : math.max(1, r.kindFacets[kinds.first.key]![1]);
    return Padding(
      padding: const EdgeInsets.fromLTRB(0, 10, 12, 12),
      child: Container(
        decoration: BoxDecoration(color: cs.surfaceContainerLow, borderRadius: BorderRadius.circular(24)),
        child: ListView(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 88),
          children: [
            Text(t('sum.title'), style: tx.titleMedium),
            const SizedBox(height: 14),
            Row(
              children: [
                Donut(
                  size: 112,
                  thickness: 12,
                  parts: [for (var i = 0; i < c.length; i++) (c[i][0].toDouble(), tiers[i].$1)],
                  center: Text(fmtCount(total), style: tx.titleMedium?.copyWith(fontWeight: FontWeight.w700)),
                ),
                const SizedBox(width: 16),
                Expanded(
                  child: Column(
                    children: [
                      for (var i = 0; i < c.length; i++)
                        Padding(
                          padding: const EdgeInsets.symmetric(vertical: 3),
                          child: Row(
                            children: [
                              Container(width: 10, height: 10, decoration: BoxDecoration(color: tiers[i].$1, borderRadius: BorderRadius.circular(4))),
                              const SizedBox(width: 8),
                              Expanded(child: Text(tiers[i].$3, style: tx.labelMedium, overflow: TextOverflow.ellipsis)),
                              Text(fmtCount(c[i][0]), style: tx.labelMedium?.copyWith(color: cs.onSurfaceVariant)),
                            ],
                          ),
                        ),
                    ],
                  ),
                ),
              ],
            ),
            const SizedBox(height: 14),
            FilledButton.tonalIcon(
              onPressed: total == 0 ? null : r.selectRecoverable,
              icon: const Icon(Icons.done_all_rounded, size: 18),
              label: Text(t('sum.select')),
            ),
            const SizedBox(height: 20),
            Text(t('sum.types'), style: tx.titleSmall),
            const SizedBox(height: 8),
            for (final k in kinds)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 5),
                child: Row(
                  children: [
                    Icon(k.icon, size: 18, color: kc.of(k.index).$2),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Row(children: [
                            Expanded(child: Text(k.label, style: tx.labelMedium, overflow: TextOverflow.ellipsis)),
                            Text(fmtBytes(r.kindFacets[k.key]![1]), style: tx.labelSmall?.copyWith(color: cs.onSurfaceVariant)),
                          ]),
                          const SizedBox(height: 4),
                          StackedBar(height: 6, parts: [(r.kindFacets[k.key]![1].toDouble(), kc.solid(k.index)), ((maxB - r.kindFacets[k.key]![1]).toDouble(), Colors.transparent)]),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            const SizedBox(height: 14),
            Row(children: [
              Icon(Icons.touch_app_rounded, size: 16, color: cs.onSurfaceVariant),
              const SizedBox(width: 8),
              Expanded(child: Text(t('sum.hint'), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant))),
            ]),
          ],
        ),
      ),
    );
  }
}

class _Skeleton extends StatelessWidget {
  final bool grid;
  const _Skeleton({this.grid = false});

  @override
  Widget build(BuildContext context) {
    final c = Theme.of(context).colorScheme.surfaceContainer;
    return Padding(
      padding: grid ? EdgeInsets.zero : const EdgeInsets.symmetric(vertical: 4),
      child: DecoratedBox(decoration: BoxDecoration(color: c, borderRadius: BorderRadius.circular(16))),
    );
  }
}

class _Row extends StatelessWidget {
  final FileItem item;
  final Results r;
  final bool compact;
  const _Row({required this.item, required this.r, this.compact = false});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final sel = r.isSelected(item);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: ValueListenableBuilder<FileItem?>(
        valueListenable: r.focusN,
        builder: (_, f, child) => DecoratedBox(
          decoration: BoxDecoration(
            color: sel ? cs.primaryContainer.withValues(alpha: 0.55) : (f?.id == item.id ? cs.surfaceContainerHigh : Colors.transparent),
            borderRadius: BorderRadius.circular(16),
          ),
          child: child,
        ),
        child: Material(
          color: Colors.transparent,
          borderRadius: BorderRadius.circular(16),
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: () => r.focus(item),
            onDoubleTap: () => r.toggle(item),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 6),
              child: Row(
                children: [
                  IconButton(
                    onPressed: () => r.toggle(item),
                    icon: AnimatedSwitcher(
                      duration: Motion.short,
                      transitionBuilder: (c, a) => ScaleTransition(scale: a, child: c),
                      child: KeyedSubtree(key: ValueKey(sel), child: symbolsCheck(sel, sel ? cs.primary : cs.outline)),
                    ),
                  ),
                  FileThumb(item: item),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(item.name, style: tx.bodyLarge, maxLines: 1, overflow: TextOverflow.ellipsis),
                        Text(item.path.isEmpty ? item.state.label : pathLabel(item.path),
                            style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant), maxLines: 1, overflow: TextOverflow.ellipsis),
                      ],
                    ),
                  ),
                  const SizedBox(width: 10),
                  if (item.state != FileState.deleted)
                    Tooltip(message: item.state.label, child: Padding(padding: const EdgeInsets.only(right: 8), child: Icon(item.state.icon, size: 18, color: cs.onSurfaceVariant))),
                  SizedBox(width: 78, child: Text(fmtBytes(item.size), textAlign: TextAlign.end, style: tx.bodyMedium)),
                  if (!compact) ...[
                    const SizedBox(width: 14),
                    SizedBox(
                      width: 92,
                      child: Text(fmtDate(item.bestDate, time: false), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant), maxLines: 1),
                    ),
                  ],
                  SizedBox(width: 62, child: Align(alignment: AlignmentDirectional.centerEnd, child: ChanceBadge(chance: item.chance))),
                  const SizedBox(width: 6),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _GridTile extends StatelessWidget {
  final FileItem item;
  final Results r;
  const _GridTile({required this.item, required this.r});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final sel = r.isSelected(item);
    return Pressable(
      color: sel ? cs.primaryContainer.withValues(alpha: 0.6) : cs.surfaceContainerLow,
      onTap: () => r.focus(item),
      child: Stack(
        children: [
          Padding(
            padding: const EdgeInsets.all(8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: LayoutBuilder(
                    builder: (_, c) => FileThumb(item: item, size: c.maxWidth, px: 320, radius: 14),
                  ),
                ),
                const SizedBox(height: 6),
                Text(item.name, style: tx.bodyMedium, maxLines: 1, overflow: TextOverflow.ellipsis),
                Row(
                  children: [
                    Expanded(child: Text(fmtBytes(item.size), style: tx.bodySmall?.copyWith(color: cs.onSurfaceVariant))),
                    ChanceBadge(chance: item.chance),
                  ],
                ),
              ],
            ),
          ),
          Positioned(
            top: 2,
            right: 2,
            child: IconButton(onPressed: () => r.toggle(item), icon: symbolsCheck(sel, sel ? cs.primary : Colors.white70)),
          ),
        ],
      ),
    );
  }
}

class _StateChips extends StatelessWidget {
  final Results r;
  const _StateChips({required this.r});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 44,
      child: ListView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(horizontal: 10),
        children: [
          for (final s in FileState.values)
            if ((r.stateFacets[s.key]?[0] ?? 0) > 0)
              Padding(
                padding: const EdgeInsetsDirectional.only(end: 6, top: 4, bottom: 4),
                child: FilterChip(
                  visualDensity: VisualDensity.compact,
                  avatar: Icon(s.icon, size: 17),
                  label: Text('${s.label}  ${fmtCount(r.stateFacets[s.key]?[0] ?? 0)}'),
                  selected: r.states.contains(s),
                  onSelected: (v) => r.change((x) {
                    final n = {...x.states};
                    v ? n.add(s) : n.remove(s);
                    x.states = n;
                  }),
                ),
              ),
          Padding(
            padding: const EdgeInsetsDirectional.only(end: 6, top: 4, bottom: 4),
            child: FilterChip(
              visualDensity: VisualDensity.compact,
              avatar: const Icon(Icons.filter_none_rounded, size: 17),
              label: Text(t('browse.hide_dupes')),
              selected: r.hideDupes,
              onSelected: (v) => r.change((x) => x.hideDupes = v),
            ),
          ),
          Padding(
            padding: const EdgeInsetsDirectional.only(end: 6, top: 4, bottom: 4),
            child: FilterChip(
              visualDensity: VisualDensity.compact,
              avatar: const Icon(Icons.verified_rounded, size: 17),
              label: Text(t('browse.recoverable')),
              selected: r.onlyRecoverable,
              onSelected: (v) => r.change((x) => x.onlyRecoverable = v),
            ),
          ),
        ],
      ),
    );
  }
}

class _SortMenu extends StatelessWidget {
  final Results r;
  const _SortMenu({required this.r});

  @override
  Widget build(BuildContext context) {
    final opts = [
      ('smart', true, t('sort.smart')),
      ('date', true, t('sort.date')),
      ('size', true, t('sort.size')),
      ('name', false, t('sort.name')),
      ('kind', false, t('sort.ext')),
    ];
    return PopupMenuButton<(String, bool)>(
      tooltip: t('browse.sort'),
      icon: const Icon(Icons.swap_vert_rounded),
      onSelected: (v) => r.change((x) {
        x.sortBy = v.$1;
        x.sortDesc = v.$2;
      }),
      itemBuilder: (_) => [
        for (final o in opts)
          CheckedPopupMenuItem(value: (o.$1, o.$2), checked: r.sortBy == o.$1, child: Text(o.$3)),
      ],
    );
  }
}

class _FilterButton extends StatelessWidget {
  final Results r;
  const _FilterButton({required this.r});

  @override
  Widget build(BuildContext context) {
    final active = r.minSize != null || r.maxSize != null || r.dateRange != null;
    return IconButton(
      tooltip: t('browse.filter'),
      isSelected: active,
      icon: Badge(isLabelVisible: active, smallSize: 8, child: const Icon(Icons.tune_rounded)),
      onPressed: () => _open(context),
    );
  }

  void _open(BuildContext context) {
    var min = (r.minSize ?? 0) / (1 << 20);
    var max = (r.maxSize ?? 0) / (1 << 20);
    var range = r.dateRange;
    showDialog<void>(
      context: context,
      builder: (c) => StatefulBuilder(
        builder: (c, set) => AlertDialog(
          title: Text(t('filter.title')),
          content: SizedBox(
            width: 420,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('${t('filter.min')}: ${min == 0 ? t('common.unlimited') : '${min.toStringAsFixed(0)} MB'}'),
                Slider(value: min, max: 500, divisions: 100, onChanged: (v) => set(() => min = v)),
                Text('${t('filter.max')}: ${max == 0 ? t('common.unlimited') : '${max.toStringAsFixed(0)} MB'}'),
                Slider(value: max, max: 4096, divisions: 128, onChanged: (v) => set(() => max = v)),
                const SizedBox(height: 8),
                OutlinedButton.icon(
                  icon: const Icon(Icons.date_range_rounded),
                  label: Text(range == null
                      ? t('filter.date')
                      : '${fmtDate(range!.start.millisecondsSinceEpoch ~/ 1000, time: false)} – ${fmtDate(range!.end.millisecondsSinceEpoch ~/ 1000, time: false)}'),
                  onPressed: () async {
                    final v = await showDateRangePicker(
                      context: c,
                      firstDate: DateTime(1990),
                      lastDate: DateTime.now().add(const Duration(days: 1)),
                      initialDateRange: range,
                    );
                    if (v != null) set(() => range = v);
                  },
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () {
                Navigator.pop(c);
                r.change((x) {
                  x.minSize = null;
                  x.maxSize = null;
                  x.dateRange = null;
                });
              },
              child: Text(t('common.clear')),
            ),
            FilledButton(
              onPressed: () {
                Navigator.pop(c);
                r.change((x) {
                  x.minSize = min > 0 ? (min * (1 << 20)).toInt() : null;
                  x.maxSize = max > 0 ? (max * (1 << 20)).toInt() : null;
                  x.dateRange = range;
                });
              },
              child: Text(t('common.apply')),
            ),
          ],
        ),
      ),
    );
  }
}

class _Sidebar extends StatefulWidget {
  final Results r;
  const _Sidebar({required this.r});
  @override
  State<_Sidebar> createState() => _SidebarState();
}

class _SidebarState extends State<_Sidebar> {
  bool folders = false;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 10, 4, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SegmentedButton<bool>(
            showSelectedIcon: false,
            style: ButtonStyle(visualDensity: VisualDensity.compact),
            segments: [
              ButtonSegment(value: false, label: Text(t('browse.kinds'))),
              ButtonSegment(value: true, label: Text(t('browse.folders'))),
            ],
            selected: {folders},
            onSelectionChanged: (v) => setState(() => folders = v.first),
          ),
          const SizedBox(height: 8),
          Expanded(
            child: AnimatedSwitcher(
              duration: Motion.medium,
              child: folders
                  ? _FolderTree(key: const ValueKey('f'), r: widget.r)
                  : ListView(
                      key: const ValueKey('k'),
                      children: [
                        _KindTile(icon: Icons.apps_rounded, label: t('browse.all'), count: null, selected: widget.r.kinds.isEmpty, onTap: () => widget.r.change((x) => x.kinds = {})),
                        const SizedBox(height: 4),
                        for (final k in FileKind.values)
                          if ((widget.r.kindFacets[k.key]?[0] ?? 0) > 0 || widget.r.kinds.contains(k))
                            _KindTile(
                              icon: k.icon,
                              label: k.label,
                              count: widget.r.kindFacets[k.key]?[0],
                              share: (widget.r.kindFacets[k.key]?[1] ?? 0) / math.max(1, widget.r.kindFacets.values.fold<int>(0, (a, b) => a + b[1])),
                              tone: Theme.of(context).extension<KindColors>()!.of(k.index),
                              selected: widget.r.kinds.contains(k),
                              onTap: () => widget.r.change((x) => x.kinds = x.kinds.contains(k) ? {} : {k}),
                            ),
                      ],
                    ),
            ),
          ),
          if (widget.r.dirId != null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: InputChip(
                avatar: Icon(Icons.folder_rounded, size: 18, color: cs.primary),
                label: Text(widget.r.dirName ?? t('browse.folder'), overflow: TextOverflow.ellipsis),
                onDeleted: () => widget.r.change((x) {
                  x.dirId = null;
                  x.dirName = null;
                }),
              ),
            ),
        ],
      ),
    );
  }
}

class _KindTile extends StatelessWidget {
  final IconData icon;
  final String label;
  final int? count;
  final double? share;
  final (Color, Color)? tone;
  final bool selected;
  final VoidCallback onTap;
  const _KindTile({required this.icon, required this.label, required this.count, this.share, this.tone, required this.selected, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final fg = selected ? cs.onSecondaryContainer : cs.onSurfaceVariant;
    return Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: AnimatedContainer(
        duration: Motion.short,
        decoration: BoxDecoration(color: selected ? cs.secondaryContainer : Colors.transparent, borderRadius: BorderRadius.circular(18)),
        child: Pressable(
          onTap: onTap,
          borderRadius: BorderRadius.circular(18),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(8, 7, 12, 7),
            child: Row(
              children: [
                Container(
                  width: 32,
                  height: 32,
                  decoration: BoxDecoration(color: tone?.$1 ?? cs.surfaceContainerHighest, borderRadius: BorderRadius.circular(11)),
                  child: Icon(icon, size: 18, color: tone?.$2 ?? fg),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Expanded(child: Text(label, style: tx.labelLarge?.copyWith(color: selected ? fg : cs.onSurface), overflow: TextOverflow.ellipsis)),
                          if (count case final c?) Text(fmtCount(c), style: tx.labelSmall?.copyWith(color: fg)),
                        ],
                      ),
                      if (share case final v?) ...[
                        const SizedBox(height: 4),
                        ClipRRect(
                          borderRadius: BorderRadius.circular(2),
                          child: LinearProgressIndicator(value: v.clamp(0.02, 1.0), minHeight: 3, trackGap: 0, stopIndicatorRadius: 0, stopIndicatorColor: Colors.transparent, color: tone?.$2, backgroundColor: cs.surfaceContainerHighest),
                        ),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _FolderTree extends StatefulWidget {
  final Results r;
  const _FolderTree({super.key, required this.r});
  @override
  State<_FolderTree> createState() => _FolderTreeState();
}

class _FolderTreeState extends State<_FolderTree> {
  List<DirNode> roots = [];
  final _kids = <int, List<DirNode>>{};
  final _open = <int>{};
  bool loading = true;
  String _sig = '';

  @override
  void initState() {
    super.initState();
    widget.r.addListener(_changed);
    _load();
  }

  @override
  void dispose() {
    widget.r.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    final f = Map<String, dynamic>.of(widget.r.filter)..remove('dir');
    final sig = f.toString() + widget.r.totalCount.toString();
    if (sig != _sig) {
      _sig = sig;
      _load();
    }
  }

  Future<List<DirNode>> _fetch(int? parent) async {
    final f = Map<String, dynamic>.of(widget.r.filter)..remove('dir');
    final v = await widget.r.session.engine!.call('tree', {'filter': f, 'parent': ?parent}) as Map<String, dynamic>;
    return (v['dirs'] as List).map((e) => DirNode.fromJson(e as Map<String, dynamic>)).toList();
  }

  Future<void> _load() async {
    try {
      var top = await _fetch(null);
      // Skip the root node if there is a single volume.
      if (top.length == 1) top = await _fetch(top.first.id);
      final open = _open.toList();
      final kids = <int, List<DirNode>>{};
      for (final o in open) {
        kids[o] = await _fetch(o);
      }
      if (!mounted) return;
      setState(() {
        roots = top;
        _kids
          ..clear()
          ..addAll(kids);
        loading = false;
      });
    } catch (_) {}
  }

  Future<void> _toggle(DirNode d) async {
    if (_open.remove(d.id)) {
      setState(() {});
      return;
    }
    final k = await _fetch(d.id);
    if (!mounted) return;
    setState(() {
      _open.add(d.id);
      _kids[d.id] = k;
    });
  }

  List<(DirNode, int)> get _flat {
    final out = <(DirNode, int)>[];
    void walk(List<DirNode> l, int depth) {
      for (final d in l) {
        out.add((d, depth));
        if (_open.contains(d.id)) walk(_kids[d.id] ?? const [], depth + 1);
      }
    }

    walk(roots, 0);
    return out;
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    if (loading) return const Center(child: CircularProgressIndicator());
    final list = _flat;
    if (list.isEmpty) return Center(child: Text(t('browse.no_folders'), style: tx.bodySmall));
    return ListView.builder(
      itemCount: list.length,
      itemExtent: 38,
      itemBuilder: (_, i) {
        final (d, depth) = list[i];
        final sel = widget.r.dirId == d.id;
        return Padding(
          padding: EdgeInsets.only(left: depth * 12.0),
          child: Material(
            color: sel ? cs.secondaryContainer : Colors.transparent,
            borderRadius: BorderRadius.circular(12),
            clipBehavior: Clip.antiAlias,
            child: InkWell(
              onTap: () => widget.r.change((x) {
                x.dirId = sel ? null : d.id;
                x.dirName = sel ? null : d.name;
              }),
              child: Row(
                children: [
                  SizedBox(
                    width: 26,
                    child: d.hasChildren
                        ? InkWell(
                            onTap: () => _toggle(d),
                            child: AnimatedRotation(
                              turns: _open.contains(d.id) ? 0.25 : 0,
                              duration: Motion.short,
                              child: const Icon(Icons.chevron_right_rounded, size: 18),
                            ),
                          )
                        : null,
                  ),
                  Icon(d.deleted ? Icons.folder_delete_rounded : Icons.folder_rounded, size: 18, color: d.deleted ? cs.tertiary : cs.onSurfaceVariant),
                  const SizedBox(width: 6),
                  Expanded(child: Text(dirLabel(d.name), style: tx.bodySmall, maxLines: 1, overflow: TextOverflow.ellipsis)),
                  Padding(padding: const EdgeInsets.only(right: 8, left: 4), child: Text(fmtCount(d.count), style: tx.labelSmall?.copyWith(color: cs.onSurfaceVariant))),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

class _SelectionBar extends StatelessWidget {
  final Results r;
  final ScanSession session;
  const _SelectionBar({required this.r, required this.session});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final tx = Theme.of(context).textTheme;
    final show = r.selectionCount > 0;
    return IgnorePointer(
      ignoring: !show,
      child: AnimatedSlide(
        offset: show ? Offset.zero : const Offset(0, 2.2),
        duration: Motion.long,
        curve: show ? Motion.spring : Motion.exit,
        child: Visibility(
          visible: show,
          maintainState: true,
          maintainAnimation: true,
          maintainSize: true,
          child: Center(
            child: Material(
              color: cs.inverseSurface,
              elevation: 6,
              shadowColor: Colors.black54,
              borderRadius: BorderRadius.circular(32),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(22, 8, 8, 8),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(t('browse.selected', {'n': fmtCount(r.selectionCount), 'b': fmtBytes(r.selectionBytes)}), style: tx.titleSmall?.copyWith(color: cs.onInverseSurface)),
                    const SizedBox(width: 12),
                    IconButton(
                      tooltip: t('browse.unselect'),
                      color: cs.onInverseSurface,
                      onPressed: r.clearSelection,
                      icon: const Icon(Icons.close_rounded),
                    ),
                    const SizedBox(width: 4),
                    FilledButton.icon(
                      onPressed: () => showRecover(context, session, r),
                      style: FilledButton.styleFrom(minimumSize: const Size(0, 48)),
                      icon: const Icon(Icons.restore_page_rounded),
                      label: Text(t('browse.recover')),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Bulk actions on results.
class _ToolsMenu extends StatelessWidget {
  final Results r;
  final ScanSession session;
  const _ToolsMenu({required this.r, required this.session});

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<String>(
      tooltip: t('common.more'),
      icon: const Icon(Icons.more_horiz_rounded),
      onSelected: (v) {
        switch (v) {
          case 'grep':
            showGrep(context, session, r);
          case 'validate':
            showValidate(context, session, r);
          case 'csv' || 'json' || 'html':
            exportList(context, session, r, v);
          case 'keys':
            showShortcuts(context);
        }
      },
      itemBuilder: (_) => [
        _item('grep', Icons.find_in_page_rounded, t('tools.grep')),
        _item('validate', Icons.verified_user_rounded, t('tools.validate')),
        const PopupMenuDivider(),
        _item('csv', Icons.table_view_rounded, t('tools.csv')),
        _item('json', Icons.data_object_rounded, t('tools.json')),
        _item('html', Icons.html_rounded, t('tools.html')),
        const PopupMenuDivider(),
        _item('keys', Icons.keyboard_rounded, t('tools.keys')),
      ],
    );
  }

  PopupMenuItem<String> _item(String v, IconData i, String s) =>
      PopupMenuItem(value: v, child: ListTile(leading: Icon(i), title: Text(s), contentPadding: EdgeInsets.zero, dense: true));
}
