import 'dart:math' as math;

import 'package:flutter/material.dart';

const _font = 'Roboto';

/// Recovery chance colors: matched to the seed color, readable in both themes.
@immutable
class StatusColors extends ThemeExtension<StatusColors> {
  final Color good, onGood, fair, onFair, poor, onPoor, lost, onLost;
  const StatusColors({
    required this.good,
    required this.onGood,
    required this.fair,
    required this.onFair,
    required this.poor,
    required this.onPoor,
    required this.lost,
    required this.onLost,
  });

  factory StatusColors.from(Brightness b) {
    ColorScheme s(Color c) => ColorScheme.fromSeed(seedColor: c, brightness: b);
    final g = s(const Color(0xFF2E9E5B));
    final y = s(const Color(0xFFD9A400));
    final o = s(const Color(0xFFE2702B));
    final r = s(const Color(0xFFD93A4A));
    return StatusColors(
      good: g.primaryContainer,
      onGood: g.onPrimaryContainer,
      fair: y.primaryContainer,
      onFair: y.onPrimaryContainer,
      poor: o.primaryContainer,
      onPoor: o.onPrimaryContainer,
      lost: r.errorContainer,
      onLost: r.onErrorContainer,
    );
  }

  (Color, Color) tier(int t) => switch (t) {
        3 => (good, onGood),
        2 => (fair, onFair),
        1 => (poor, onPoor),
        _ => (lost, onLost),
      };

  @override
  StatusColors copyWith() => this;

  @override
  StatusColors lerp(ThemeExtension<StatusColors>? other, double t) {
    if (other is! StatusColors) return this;
    Color l(Color a, Color b) => Color.lerp(a, b, t)!;
    return StatusColors(
      good: l(good, other.good),
      onGood: l(onGood, other.onGood),
      fair: l(fair, other.fair),
      onFair: l(onFair, other.onFair),
      poor: l(poor, other.poor),
      onPoor: l(onPoor, other.onPoor),
      lost: l(lost, other.lost),
      onLost: l(onLost, other.onLost),
    );
  }
}

/// Each file type has its own tone: lists read at a glance, the screen is not one color.
@immutable
class KindColors extends ThemeExtension<KindColors> {
  final List<(Color, Color)> pairs;
  const KindColors(this.pairs);

  static const _base = [
    Color(0xFF4C8DFF), // image
    Color(0xFFB566F2), // video
    Color(0xFFF2649B), // audio
    Color(0xFFF2A93B), // document
    Color(0xFFC08A5A), // archive
    Color(0xFF3CC4A0), // code
    Color(0xFF7C86F5), // application
    Color(0xFF38BDF8), // database
    Color(0xFF8A94A6), // other
  ];

  factory KindColors.from(Brightness b, Color surface) {
    final dark = b == Brightness.dark;
    return KindColors([
      for (final c in _base)
        (Color.lerp(surface, c, dark ? 0.24 : 0.2)!, Color.lerp(c, dark ? Colors.white : Colors.black, dark ? 0.35 : 0.4)!),
    ]);
  }

  (Color, Color) of(int kindIndex) => pairs[kindIndex];
  Color solid(int kindIndex) => _base[kindIndex];

  @override
  KindColors copyWith() => this;

  @override
  KindColors lerp(ThemeExtension<KindColors>? other, double t) => other is KindColors ? (t < 0.5 ? this : other) : this;
}

final _themes = <(Color, Brightness), ThemeData>{};

/// ColorScheme.fromSeed is expensive; the same seed is not recomputed.
ThemeData themeFor(Color seed, Brightness b) => _themes.putIfAbsent((seed, b), () => buildTheme(seed, b));

ThemeData buildTheme(Color seed, Brightness b) {
  final cs = ColorScheme.fromSeed(seedColor: seed, brightness: b, dynamicSchemeVariant: DynamicSchemeVariant.vibrant);
  final base = ThemeData(
    useMaterial3: true,
    colorScheme: cs,
    fontFamily: _font,
    // Non-Latin scripts come from system fonts.
    fontFamilyFallback: const ['Noto Sans', 'Noto Sans CJK SC', 'Noto Sans Arabic', 'Noto Sans Devanagari', 'Noto Sans Thai', 'DejaVu Sans'],
    brightness: b,
  );
  final t = base.textTheme;
  TextStyle? tight(TextStyle? s, double ls) => s?.copyWith(letterSpacing: ls);
  return base.copyWith(
    scaffoldBackgroundColor: cs.surface,
    textTheme: t.copyWith(
      displaySmall: tight(t.displaySmall, -0.8)?.copyWith(fontWeight: FontWeight.w500),
      headlineLarge: tight(t.headlineLarge, -0.6)?.copyWith(fontWeight: FontWeight.w500),
      headlineMedium: tight(t.headlineMedium, -0.4)?.copyWith(fontWeight: FontWeight.w500),
      headlineSmall: tight(t.headlineSmall, -0.2)?.copyWith(fontWeight: FontWeight.w500),
      titleLarge: t.titleLarge?.copyWith(fontWeight: FontWeight.w500),
      titleMedium: t.titleMedium?.copyWith(fontWeight: FontWeight.w500),
    ),
    extensions: [StatusColors.from(b), KindColors.from(b, cs.surface)],
    splashFactory: InkRipple.splashFactory,
    // ignore: deprecated_member_use
    sliderTheme: const SliderThemeData(year2023: false),
    // ignore: deprecated_member_use
    progressIndicatorTheme: const ProgressIndicatorThemeData(year2023: false),
    cardTheme: CardThemeData(
      elevation: 0,
      margin: EdgeInsets.zero,
      color: cs.surfaceContainerLow,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(24)),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: cs.surfaceContainerHigh,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(28)),
    ),
    chipTheme: base.chipTheme.copyWith(
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
      side: BorderSide(color: cs.outlineVariant),
      showCheckmark: false,
    ),
    tooltipTheme: TooltipThemeData(
      waitDuration: const Duration(milliseconds: 450),
      decoration: BoxDecoration(color: cs.inverseSurface, borderRadius: BorderRadius.circular(8)),
      textStyle: TextStyle(color: cs.onInverseSurface, fontSize: 12, fontFamily: _font),
    ),
    navigationRailTheme: NavigationRailThemeData(
      backgroundColor: cs.surfaceContainerLow,
      indicatorColor: cs.secondaryContainer,
      labelType: NavigationRailLabelType.all,
      selectedLabelTextStyle: TextStyle(color: cs.onSurface, fontWeight: FontWeight.w600, fontSize: 12, fontFamily: _font),
      unselectedLabelTextStyle: TextStyle(color: cs.onSurfaceVariant, fontSize: 12, fontFamily: _font),
    ),
    scrollbarTheme: ScrollbarThemeData(
      radius: const Radius.circular(8),
      thickness: const WidgetStatePropertyAll(6),
      thumbColor: WidgetStatePropertyAll(cs.onSurfaceVariant.withValues(alpha: 0.35)),
    ),
    snackBarTheme: SnackBarThemeData(
      behavior: SnackBarBehavior.floating,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      width: 480,
    ),
    dividerTheme: DividerThemeData(color: cs.outlineVariant.withValues(alpha: 0.5), space: 1, thickness: 1),
  );
}

/// Material 3 emphasized motion curves and a spring-like overshoot.
class Motion {
  static const enter = Easing.emphasizedDecelerate;
  static const exit = Easing.emphasizedAccelerate;
  static const standard = Easing.standard;
  static const spring = _Spring();
  static bool reduced = false;
  static Duration d(Duration x) => reduced ? Duration.zero : x;
  static Duration get short => d(const Duration(milliseconds: 160));
  static Duration get medium => d(const Duration(milliseconds: 280));
  static Duration get long => d(const Duration(milliseconds: 420));
}

/// Light overshooting spring: softens start and stop, computed in one line.
class _Spring extends Curve {
  const _Spring();
  @override
  double transformInternal(double t) => 1 - math.pow(math.e, -7.5 * t) * math.cos(10.5 * t);
}
