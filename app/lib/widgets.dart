import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'format.dart';
import 'models.dart';
import 'state.dart';
import 'theme.dart';

/// Material 3 Expressive wavy circular progress. The animation spins only while
/// active; when paused or finished it produces no frames.
class WavyRing extends StatefulWidget {
  final double? value;
  final double size;
  final bool active;
  final Widget? child;
  const WavyRing({super.key, this.value, this.size = 168, this.active = true, this.child});
  @override
  State<WavyRing> createState() => _WavyRingState();
}

class _WavyRingState extends State<WavyRing> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: const Duration(milliseconds: 2400));

  @override
  void initState() {
    super.initState();
    if (widget.active) _c.repeat();
  }

  @override
  void didUpdateWidget(WavyRing old) {
    super.didUpdateWidget(old);
    if (widget.active && !_c.isAnimating) _c.repeat();
    if (!widget.active && _c.isAnimating) _c.stop();
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return TweenAnimationBuilder<double>(
      tween: Tween(end: widget.value ?? 0.12),
      duration: Motion.long,
      curve: Motion.enter,
      builder: (_, v, child) => AnimatedBuilder(
        animation: _c,
        builder: (_, child) => CustomPaint(
          size: Size.square(widget.size),
          painter: _RingPainter(
            value: v,
            phase: _c.value,
            color: cs.primary,
            track: cs.secondaryContainer,
            spin: widget.value == null,
            wave: widget.active ? 1 : 0,
          ),
          child: child,
        ),
        child: child,
      ),
      child: SizedBox.square(dimension: widget.size, child: Center(child: widget.child)),
    );
  }
}

class _RingPainter extends CustomPainter {
  final double value, phase, wave;
  final Color color, track;
  final bool spin;
  _RingPainter({required this.value, required this.phase, required this.color, required this.track, required this.spin, required this.wave});

  @override
  void paint(Canvas canvas, Size s) {
    final c = s.center(Offset.zero);
    const stroke = 9.0;
    final r = s.width / 2 - stroke - 4;
    final start = -math.pi / 2 + (spin ? phase * 2 * math.pi : 0);
    final sweep = 2 * math.pi * value.clamp(0.0, 1.0);
    const gap = 0.16;
    final track0 = start + sweep + gap;
    final trackSweep = 2 * math.pi - sweep - gap * 2;
    if (trackSweep > 0) {
      canvas.drawArc(Rect.fromCircle(center: c, radius: r), track0, trackSweep, false,
          Paint()
            ..color = track
            ..style = PaintingStyle.stroke
            ..strokeWidth = stroke
            ..strokeCap = StrokeCap.round);
    }
    if (sweep <= 0.001) return;
    final path = Path();
    const waves = 12;
    final amp = 3.2 * wave;
    final steps = (sweep * 60).ceil().clamp(8, 400);
    for (var i = 0; i <= steps; i++) {
      final t = i / steps;
      final a = start + sweep * t;
      // The wave fades at the ends so the ring start and end stay smooth.
      final env = math.min(1.0, math.min(t, 1 - t) * 12);
      final rr = r + amp * env * math.sin(a * waves - phase * 2 * math.pi * 3);
      final p = c + Offset(math.cos(a), math.sin(a)) * rr;
      i == 0 ? path.moveTo(p.dx, p.dy) : path.lineTo(p.dx, p.dy);
    }
    canvas.drawPath(path,
        Paint()
          ..color = color
          ..style = PaintingStyle.stroke
          ..strokeWidth = stroke
          ..strokeCap = StrokeCap.round
          ..strokeJoin = StrokeJoin.round);
  }

  @override
  bool shouldRepaint(_RingPainter o) => o.value != value || o.phase != phase || o.color != color || o.wave != wave;
}

/// Live disk map of a scan.
class DiskMap extends StatelessWidget {
  final String cells;
  const DiskMap({super.key, required this.cells});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return RepaintBoundary(
      child: CustomPaint(
        painter: _MapPainter(cells, [
          cs.surfaceContainerHighest,
          cs.primary.withValues(alpha: 0.55),
          cs.outlineVariant.withValues(alpha: 0.65),
          cs.tertiary,
          cs.secondary,
          cs.error,
        ]),
        size: Size.infinite,
      ),
    );
  }
}

class _MapPainter extends CustomPainter {
  final String cells;
  final List<Color> colors;
  _MapPainter(this.cells, this.colors);

  @override
  void paint(Canvas canvas, Size s) {
    const n = 1024;
    final cols = math.max(8, math.sqrt(n * s.width / math.max(1, s.height)).floor());
    final rows = (n / cols).ceil();
    final cw = s.width / cols;
    final ch = s.height / rows;
    final side = math.min(cw, ch);
    final paint = Paint();
    final rrect = Radius.circular(side * 0.26);
    for (var i = 0; i < n; i++) {
      final v = i < cells.length ? cells.codeUnitAt(i) - 48 : 0;
      paint.color = colors[v.clamp(0, colors.length - 1)];
      final x = (i % cols) * cw;
      final y = (i ~/ cols) * ch;
      canvas.drawRRect(RRect.fromRectAndRadius(Rect.fromLTWH(x + 1, y + 1, cw - 2, ch - 2), rrect), paint);
    }
  }

  @override
  bool shouldRepaint(_MapPainter o) => o.cells != cells || o.colors[1] != colors[1];
}

class ChanceBadge extends StatelessWidget {
  final int chance;
  final bool dense;
  const ChanceBadge({super.key, required this.chance, this.dense = true});

  @override
  Widget build(BuildContext context) {
    final info = ChanceInfo.of(chance);
    final st = Theme.of(context).extension<StatusColors>()!;
    final (bg, fg) = st.tier(info.tier);
    return Container(
      padding: EdgeInsets.symmetric(horizontal: dense ? 8 : 12, vertical: dense ? 3 : 6),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(8)),
      child: Text(info.label, style: TextStyle(color: fg, fontSize: dense ? 11.5 : 13, fontWeight: FontWeight.w600)),
    );
  }
}

/// Counts smoothly as the number changes.
class RollingCount extends StatelessWidget {
  final int value;
  final TextStyle? style;
  final String Function(num) format;
  const RollingCount({super.key, required this.value, this.style, this.format = fmtCount});

  @override
  Widget build(BuildContext context) {
    return TweenAnimationBuilder<double>(
      tween: Tween(end: value.toDouble()),
      duration: Motion.medium,
      curve: Curves.easeOutCubic,
      builder: (_, v, _) => Text(format(v), style: style?.copyWith(fontFeatures: const [FontFeature.tabularFigures()])),
    );
  }
}

/// Staggered entrance when a list first opens.
class StaggerIn extends StatefulWidget {
  final int index;
  final Widget child;
  final double dy;
  const StaggerIn({super.key, required this.index, required this.child, this.dy = 18});
  @override
  State<StaggerIn> createState() => _StaggerInState();
}

class _StaggerInState extends State<StaggerIn> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: Motion.long);

  @override
  void initState() {
    super.initState();
    Future.delayed(Motion.d(Duration(milliseconds: 40 * widget.index.clamp(0, 8))), () {
      if (mounted) _c.forward();
    });
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final a = CurvedAnimation(parent: _c, curve: Motion.enter);
    if (Motion.reduced) return widget.child;
    return AnimatedBuilder(
      animation: a,
      builder: (_, child) => Transform.translate(offset: Offset(0, widget.dy * (1 - a.value)), child: child),
      child: widget.child,
    );
  }
}

/// A surface that sinks in slightly when pressed.
class Pressable extends StatefulWidget {
  final Widget child;
  final VoidCallback? onTap;
  final BorderRadius borderRadius;
  final Color? color;
  final Color? hoverColor;
  const Pressable({super.key, required this.child, this.onTap, this.borderRadius = const BorderRadius.all(Radius.circular(20)), this.color, this.hoverColor});
  @override
  State<Pressable> createState() => _PressableState();
}

class _PressableState extends State<Pressable> {
  bool _down = false;

  @override
  Widget build(BuildContext context) {
    return AnimatedScale(
      scale: _down ? 0.975 : 1,
      duration: Motion.medium,
      curve: Motion.spring,
      child: Material(
        color: widget.color ?? Colors.transparent,
        borderRadius: widget.borderRadius,
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: widget.onTap,
          hoverColor: widget.hoverColor,
          onHighlightChanged: (v) => setState(() => _down = v),
          child: widget.child,
        ),
      ),
    );
  }
}

class InfoBanner extends StatelessWidget {
  final IconData icon;
  final String title;
  final String? body;
  final Color? color;
  final Color? onColor;
  final Widget? action;
  const InfoBanner({super.key, required this.icon, required this.title, this.body, this.color, this.onColor, this.action});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final t = Theme.of(context).textTheme;
    final fg = onColor ?? cs.onSecondaryContainer;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(color: color ?? cs.secondaryContainer, borderRadius: BorderRadius.circular(18)),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: fg, size: 22),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(title, style: t.titleSmall?.copyWith(color: fg)),
                if (body != null) ...[
                  const SizedBox(height: 2),
                  Text(body!, style: t.bodySmall?.copyWith(color: fg.withValues(alpha: 0.86), height: 1.35)),
                ],
                if (action != null) ...[const SizedBox(height: 8), action!],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class EmptyState extends StatelessWidget {
  final IconData icon;
  final String title;
  final String? body;
  final Widget? action;
  const EmptyState({super.key, required this.icon, required this.title, this.body, this.action});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final t = Theme.of(context).textTheme;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 380),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 88,
              height: 88,
              decoration: BoxDecoration(color: cs.surfaceContainerHigh, borderRadius: BorderRadius.circular(32)),
              child: Icon(icon, size: 40, color: cs.onSurfaceVariant),
            ),
            const SizedBox(height: 18),
            Text(title, style: t.titleLarge, textAlign: TextAlign.center),
            if (body != null) ...[
              const SizedBox(height: 6),
              Text(body!, style: t.bodyMedium?.copyWith(color: cs.onSurfaceVariant, height: 1.4), textAlign: TextAlign.center),
            ],
            if (action != null) ...[const SizedBox(height: 18), action!],
          ],
        ),
      ),
    );
  }
}

/// A thumbnail or type icon. Requests a build if it stays visible, and does no
/// needless work for rows flying past while scrolling.
class FileThumb extends StatefulWidget {
  final FileItem item;
  final double size;
  final int px;
  final double radius;
  const FileThumb({super.key, required this.item, this.size = 40, this.px = 160, this.radius = 10});
  @override
  State<FileThumb> createState() => _FileThumbState();
}

class _FileThumbState extends State<FileThumb> {
  String? _path;
  bool _asked = false;
  ThumbLoader? _loader;

  bool get _wants => widget.item.previewAsImage && widget.item.chance > 3;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _loader ??= AppScope.read(context).thumbs;
    _maybeLoad();
  }

  @override
  void didUpdateWidget(FileThumb old) {
    super.didUpdateWidget(old);
    if (old.item.id != widget.item.id) {
      _loader?.cancel(old.item.id, old.px);
      _path = null;
      _asked = false;
      _maybeLoad();
    }
  }

  void _maybeLoad() {
    if (_asked || !_wants || _loader == null) return;
    _asked = true;
    final id = widget.item.id;
    Future.delayed(const Duration(milliseconds: 140), () async {
      if (!mounted || widget.item.id != id) return;
      final p = await _loader!.get(id, widget.px);
      if (mounted && widget.item.id == id && p != null) setState(() => _path = p);
    });
  }

  @override
  void dispose() {
    _loader?.cancel(widget.item.id, widget.px);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tone = Theme.of(context).extension<KindColors>()!.of(widget.item.kind.index);
    final fallback = Container(
      key: const ValueKey('icon'),
      width: widget.size,
      height: widget.size,
      decoration: BoxDecoration(color: tone.$1, borderRadius: BorderRadius.circular(widget.radius)),
      child: Icon(widget.item.kind.icon, size: widget.size * 0.5, color: tone.$2),
    );
    return AnimatedSwitcher(
      duration: Motion.medium,
      child: _path == null
          ? fallback
          : ClipRRect(
              key: ValueKey(_path),
              borderRadius: BorderRadius.circular(widget.radius),
              child: Image.file(File(_path!), width: widget.size, height: widget.size, fit: BoxFit.cover, cacheWidth: widget.px,
                  gaplessPlayback: true, errorBuilder: (_, _, _) => fallback),
            ),
    );
  }
}

class Stat extends StatelessWidget {
  final String label;
  final Widget value;
  const Stat(this.label, this.value, {super.key});

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context).textTheme;
    final cs = Theme.of(context).colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(label, style: t.labelMedium?.copyWith(color: cs.onSurfaceVariant)),
        const SizedBox(height: 2),
        DefaultTextStyle.merge(style: t.titleLarge, child: value),
      ],
    );
  }
}

Widget symbolsCheck(bool on, Color color) =>
    Icon(on ? Icons.check_circle_rounded : Icons.radio_button_unchecked_rounded, color: color, size: 22);

/// Completion mark: the ring fills and a tick is drawn.
class SuccessMark extends StatefulWidget {
  final bool ok;
  const SuccessMark({super.key, required this.ok});
  @override
  State<SuccessMark> createState() => _SuccessMarkState();
}

class _SuccessMarkState extends State<SuccessMark> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: Motion.d(const Duration(milliseconds: 620)))..forward();

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final bg = widget.ok ? cs.primaryContainer : cs.errorContainer;
    final fg = widget.ok ? cs.onPrimaryContainer : cs.onErrorContainer;
    return AnimatedBuilder(
      animation: _c,
      builder: (_, _) {
        final s = Motion.spring.transform(_c.value.clamp(0.0, 0.7) / 0.7);
        return Transform.scale(
          scale: 0.6 + 0.4 * s,
          child: Container(
            width: 84,
            height: 84,
            decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(32)),
            child: CustomPaint(painter: _TickPainter(((_c.value - 0.3) / 0.7).clamp(0.0, 1.0), fg, widget.ok)),
          ),
        );
      },
    );
  }
}

class _TickPainter extends CustomPainter {
  final double v;
  final Color c;
  final bool ok;
  _TickPainter(this.v, this.c, this.ok);

  @override
  void paint(Canvas canvas, Size s) {
    final p = Paint()
      ..color = c
      ..style = PaintingStyle.stroke
      ..strokeWidth = 6
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    final path = Path();
    if (ok) {
      path
        ..moveTo(s.width * .28, s.height * .52)
        ..lineTo(s.width * .44, s.height * .68)
        ..lineTo(s.width * .74, s.height * .36);
    } else {
      path
        ..moveTo(s.width * .34, s.height * .34)
        ..lineTo(s.width * .66, s.height * .66)
        ..moveTo(s.width * .66, s.height * .34)
        ..lineTo(s.width * .34, s.height * .66);
    }
    for (final m in path.computeMetrics()) {
      canvas.drawPath(m.extractPath(0, m.length * v), p);
    }
  }

  @override
  bool shouldRepaint(_TickPainter o) => o.v != v;
}


/// Ring chart: slices sweep in once on opening.
class Donut extends StatelessWidget {
  final List<(double, Color)> parts;
  final double size, thickness;
  final Widget? center;
  const Donut({super.key, required this.parts, this.size = 120, this.thickness = 14, this.center});

  @override
  Widget build(BuildContext context) {
    final track = Theme.of(context).colorScheme.surfaceContainerHighest;
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: Motion.d(const Duration(milliseconds: 700)),
      curve: Motion.enter,
      builder: (_, v, child) => CustomPaint(
        painter: _DonutPainter(parts, v, thickness, track),
        child: SizedBox.square(dimension: size, child: child),
      ),
      child: Center(child: center),
    );
  }
}

class _DonutPainter extends CustomPainter {
  final List<(double, Color)> parts;
  final double v, w;
  final Color track;
  _DonutPainter(this.parts, this.v, this.w, this.track);

  @override
  void paint(Canvas canvas, Size s) {
    final r = Rect.fromCircle(center: s.center(Offset.zero), radius: s.width / 2 - w / 2);
    final p = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = w
      ..strokeCap = StrokeCap.round;
    canvas.drawArc(r, 0, 2 * math.pi, false, p..color = track);
    final total = parts.fold<double>(0, (a, b) => a + b.$1);
    if (total <= 0) return;
    final gap = (w * 1.15) / (s.width / 2);
    var a = -math.pi / 2;
    final visible = parts.where((e) => e.$1 > 0).length;
    for (final (val, c) in parts) {
      if (val <= 0) continue;
      final full = 2 * math.pi * val / total;
      final sweep = (full - (visible > 1 ? gap : 0)).clamp(0.02, 2 * math.pi) * v;
      canvas.drawArc(r, a + (visible > 1 ? gap / 2 : 0), sweep, false, p..color = c);
      a += full;
    }
  }

  @override
  bool shouldRepaint(_DonutPainter o) => o.v != v || o.parts != parts || o.track != track;
}

/// Proportional bar: for partitions, usage and distributions.
class StackedBar extends StatelessWidget {
  final List<(double, Color)> parts;
  final double height;
  const StackedBar({super.key, required this.parts, this.height = 10});

  @override
  Widget build(BuildContext context) {
    final total = parts.fold<double>(0, (a, b) => a + b.$1);
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: Motion.long,
      curve: Motion.enter,
      builder: (_, v, _) => ClipRRect(
        borderRadius: BorderRadius.circular(height / 2),
        child: SizedBox(
          height: height,
          child: ColoredBox(
            color: Theme.of(context).colorScheme.surfaceContainerHighest,
            child: Row(
              children: [
                for (final (val, c) in parts)
                  if (val > 0)
                    Flexible(
                      flex: math.max(1, (val / math.max(total, 1) * 1000 * v).round()),
                      child: Padding(padding: const EdgeInsetsDirectional.only(end: 2), child: ColoredBox(color: c, child: const SizedBox.expand())),
                    ),
                if (v < 1) Flexible(flex: math.max(1, (1000 * (1 - v)).round()), child: const SizedBox.expand()),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Colored box with a large number.
class StatTile extends StatelessWidget {
  final IconData icon;
  final String label;
  final Widget value;
  final Color bg, fg;
  const StatTile({super.key, required this.icon, required this.label, required this.value, required this.bg, required this.fg});

  @override
  Widget build(BuildContext context) {
    final tx = Theme.of(context).textTheme;
    return Container(
      height: 86,
      padding: const EdgeInsets.symmetric(horizontal: 14),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(24)),
      child: Row(
        children: [
          Container(
            width: 42,
            height: 42,
            decoration: BoxDecoration(color: fg.withValues(alpha: 0.14), borderRadius: BorderRadius.circular(14)),
            child: Icon(icon, size: 22, color: fg),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                DefaultTextStyle.merge(
                  style: tx.titleLarge?.copyWith(color: fg, fontWeight: FontWeight.w700),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  child: value,
                ),
                Text(label, style: tx.labelSmall?.copyWith(color: fg.withValues(alpha: 0.85), height: 1.15), maxLines: 2, overflow: TextOverflow.ellipsis),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class SectionTitle extends StatelessWidget {
  final String title;
  final Widget? action;
  const SectionTitle(this.title, {super.key, this.action});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsetsDirectional.only(start: 4, bottom: 10),
      child: Row(
        children: [
          Expanded(child: Text(title, style: Theme.of(context).textTheme.titleMedium)),
          ?action,
        ],
      ),
    );
  }
}

/// Top banner decoration: nested scan rings.
class RadarArt extends StatelessWidget {
  final Color color;
  const RadarArt({super.key, required this.color});

  @override
  Widget build(BuildContext context) {
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: Motion.d(const Duration(milliseconds: 900)),
      curve: Motion.enter,
      builder: (context, v, _) => CustomPaint(painter: _RadarPainter(color, v, Directionality.of(context) == TextDirection.rtl), size: Size.infinite),
    );
  }
}

class _RadarPainter extends CustomPainter {
  final Color c;
  final double v;
  final bool rtl;
  _RadarPainter(this.c, this.v, this.rtl);

  @override
  void paint(Canvas canvas, Size s) {
    final o = Offset(s.width * (rtl ? 0.28 : 0.72), s.height * 0.5);
    final p = Paint()
      ..style = PaintingStyle.stroke
      ..strokeCap = StrokeCap.round;
    for (var i = 0; i < 5; i++) {
      final r = (46.0 + i * 46) * (0.82 + 0.18 * v);
      p
        ..strokeWidth = 16 - i * 2.4
        ..color = c.withValues(alpha: 0.2 - i * 0.03);
      final start = -0.6 + i * 1.1;
      canvas.drawArc(Rect.fromCircle(center: o, radius: r), start, (4.6 - i * 0.5) * v, false, p);
    }
    canvas.drawCircle(o, 22 * v, Paint()..color = c.withValues(alpha: 0.42));
    canvas.drawCircle(o, 9 * v, Paint()..color = c);
    final dots = [(1.1, 92.0), (3.4, 138.0), (5.2, 184.0), (0.2, 184.0), (2.3, 46.0)];
    final dp = Paint()..color = c.withValues(alpha: 0.75);
    for (final (a, r) in dots) {
      canvas.drawCircle(o + Offset(math.cos(a), math.sin(a)) * r, 5 * v, dp);
    }
  }

  @override
  bool shouldRepaint(_RadarPainter o) => o.v != v || o.c != c || o.rtl != rtl;
}
