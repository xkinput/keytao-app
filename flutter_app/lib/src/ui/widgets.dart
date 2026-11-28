import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'theme.dart';

class SchemaLogo extends StatelessWidget {
  const SchemaLogo({
    super.key,
    required this.controller,
    required this.size,
    this.schemeKey,
    this.appLogoFallback = true,
  });
  final AppController controller;
  final double size;
  final String? schemeKey;
  final bool appLogoFallback;

  @override
  Widget build(BuildContext context) {
    Widget fallback() => appLogoFallback
        ? Image.asset(
            logoAsset,
            width: size,
            height: size,
            filterQuality: FilterQuality.medium,
            excludeFromSemantics: true,
            gaplessPlayback: true,
          )
        : const SizedBox.shrink();
    final key = schemeKey;
    final path = key != null
        ? controller.effectiveSchemeLogoPath(key)
        : controller.activeSchemaLogoPath ??
              controller
                  .installedPackageFor(controller.activeSchemeKey)
                  ?.logoPath;
    if (path == null) return fallback();
    final file = File(path);
    final version = controller.schemaLogoVersion(path);
    return RepaintBoundary(
      child: Image(
        image: version == null
            ? FileImage(file)
            : _VersionedLogoImage(file, version),
        width: size,
        height: size,
        filterQuality: FilterQuality.medium,
        excludeFromSemantics: true,
        gaplessPlayback: true,
        errorBuilder: (_, _, _) => fallback(),
      ),
    );
  }
}

// Rendered active logos and remote previews can change without changing their path.
class _VersionedLogoImage extends FileImage {
  const _VersionedLogoImage(super.file, this.version);

  final Object version;

  @override
  bool operator ==(Object other) =>
      other is _VersionedLogoImage &&
      other.file.path == file.path &&
      other.version == version;

  @override
  int get hashCode => Object.hash(file.path, version);
}

class AppSystemBars extends StatelessWidget {
  const AppSystemBars({super.key, required this.child});
  final Widget child;

  static SystemUiOverlayStyle styleOf(BuildContext context) =>
      (context.theme.colors.brightness == Brightness.dark
              ? SystemUiOverlayStyle.light
              : SystemUiOverlayStyle.dark)
          .copyWith(
            statusBarColor: Colors.transparent,
            systemNavigationBarColor: Colors.transparent,
            systemNavigationBarContrastEnforced: false,
          );

  @override
  Widget build(BuildContext context) => AnnotatedRegion<SystemUiOverlayStyle>(
    value: styleOf(context),
    child: child,
  );
}

class EyebrowLabel extends StatelessWidget {
  const EyebrowLabel(this.label, {super.key});
  final String label;

  @override
  Widget build(BuildContext context) => Semantics(
    header: true,
    child: Text(
      label,
      style: context.theme.typography.body.sm.copyWith(
        color: AppPalette.of(context).muted,
        fontSize: 13,
        fontWeight: FontWeight.w600,
        letterSpacing: .52,
      ),
    ),
  );
}

class BigTextRadioGroup<T> extends StatelessWidget {
  const BigTextRadioGroup({
    super.key,
    required this.label,
    required this.value,
    required this.options,
    this.onChanged,
    this.disabled = const {},
    this.semanticLabels = const {},
  });
  final String label;
  final T value;
  final Map<T, String> options;
  final ValueChanged<T>? onChanged;
  final Set<T> disabled;
  final Map<T, String> semanticLabels;

  @override
  Widget build(BuildContext context) => Semantics(
    label: label,
    child: RadioGroup<T>(
      groupValue: value,
      onChanged: (selected) {
        if (selected case final T option) onChanged?.call(option);
      },
      child: Wrap(
        spacing: MediaQuery.sizeOf(context).width >= 720 ? 15 : 14,
        runSpacing: 4,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          for (final entry in options.entries)
            _BigTextRadio<T>(
              key: ValueKey(entry.key),
              value: entry.key,
              label: entry.value,
              semanticLabel: semanticLabels[entry.key] ?? entry.value,
              selected: entry.key == value,
              enabled: onChanged != null && !disabled.contains(entry.key),
            ),
        ],
      ),
    ),
  );
}

class _BigTextRadio<T> extends StatefulWidget {
  const _BigTextRadio({
    super.key,
    required this.value,
    required this.label,
    required this.semanticLabel,
    required this.selected,
    required this.enabled,
  });
  final T value;
  final String label;
  final String semanticLabel;
  final bool selected;
  final bool enabled;

  @override
  State<_BigTextRadio<T>> createState() => _BigTextRadioState<T>();
}

class _BigTextRadioState<T> extends State<_BigTextRadio<T>>
    with RadioClient<T> {
  @override
  final focusNode = FocusNode();
  @override
  bool get tristate => false;
  @override
  T get radioValue => widget.value;
  @override
  bool get enabled => widget.enabled;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    registry = RadioGroup.maybeOf<T>(context);
  }

  @override
  void dispose() {
    registry = null;
    focusNode.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final color = widget.selected ? palette.ink : palette.muted;
    return FTappable(
      focusNode: focusNode,
      semanticsLabel: widget.semanticLabel,
      semanticsButton: false,
      semanticsChecked: widget.selected,
      semanticsInMutuallyExclusiveGroup: true,
      excludeSemantics: true,
      focusedOutlineStyle: FFocusedOutlineStyleDelta.delta(
        color: ChromeAccent.of(context),
        borderRadius: BorderRadius.circular(4),
      ),
      shortcuts: const {
        SingleActivator(LogicalKeyboardKey.enter): ActivateIntent(),
        SingleActivator(LogicalKeyboardKey.space): ActivateIntent(),
      },
      onPress: enabled
          ? () {
              if (!widget.selected) registry?.onChanged(widget.value);
            }
          : null,
      child: ConstrainedBox(
        constraints: const BoxConstraints(minHeight: 44),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 4),
          child: Text(
            widget.label,
            style: context.theme.typography.body.sm.copyWith(
              color: enabled ? color : context.theme.colors.disable(color),
              fontSize: MediaQuery.sizeOf(context).width >= 720 ? 28 : 20,
              fontWeight: FontWeight.w700,
              height: 1.2,
            ),
          ),
        ),
      ),
    );
  }
}

class HairlineGridSection extends StatelessWidget {
  const HairlineGridSection({
    super.key,
    required this.children,
    this.minCellWidth = 320,
    this.columnCount,
  }) : assert(minCellWidth > 0),
       assert(columnCount == null || columnCount > 0);
  final List<Widget> children;
  final double minCellWidth;
  final int? columnCount;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      if (children.isEmpty) return const SizedBox.shrink();
      final columns = constraints.hasBoundedWidth
          ? columnCount ??
                (MediaQuery.sizeOf(context).width >= 720
                    ? (constraints.maxWidth / minCellWidth).floor().clamp(
                        1,
                        children.length,
                      )
                    : 1)
          : 1;
      return _HairlineMasonry(
        columns: columns,
        columnSizing: constraints.hasBoundedWidth
            ? const FlexColumnWidth()
            : const IntrinsicColumnWidth(),
        border: BorderSide(color: AppPalette.of(context).hairline),
        children: [
          for (final child in children)
            // The catalogue toggle still supplies a TableCell.
            if (child is TableCell)
              KeyedSubtree(key: child.key, child: child.child)
            else
              child,
        ],
      );
    },
  );
}

class _HairlineMasonry extends MultiChildRenderObjectWidget {
  const _HairlineMasonry({
    required this.columns,
    required this.columnSizing,
    required this.border,
    required super.children,
  });

  final int columns;
  final TableColumnWidth columnSizing;
  final BorderSide border;

  @override
  _RenderHairlineMasonry createRenderObject(BuildContext context) =>
      _RenderHairlineMasonry(columns, columnSizing, border);

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderHairlineMasonry renderObject,
  ) => renderObject.update(columns, columnSizing, border);
}

class _MasonryParentData extends ContainerBoxParentData<RenderBox> {
  late int column;
}

class _RenderHairlineMasonry extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _MasonryParentData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _MasonryParentData> {
  _RenderHairlineMasonry(this._columns, this._columnSizing, this._border);

  static const _slotTolerance = 16.0;

  int _columns;
  TableColumnWidth _columnSizing;
  BorderSide _border;
  late double _columnWidth;
  late List<double> _bottoms;

  void update(int columns, TableColumnWidth columnSizing, BorderSide border) {
    if (_columns != columns || _columnSizing != columnSizing) {
      _columns = columns;
      _columnSizing = columnSizing;
      markNeedsLayout();
    }
    if (_border != border) {
      _border = border;
      markNeedsPaint();
    }
  }

  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _MasonryParentData) {
      child.parentData = _MasonryParentData();
    }
  }

  Iterable<RenderBox> get _cells sync* {
    for (var child = firstChild; child != null; child = childAfter(child)) {
      yield child;
    }
  }

  double _widthFor(BoxConstraints constraints) => constraints.hasBoundedWidth
      ? constraints.maxWidth / _columns
      : constraints.constrainWidth(computeMaxIntrinsicWidth(double.infinity));

  List<double> _stack(
    double Function(RenderBox child, int column, double top) place,
  ) {
    final bottoms = List<double>.filled(_columns, 0);
    for (final child in _cells) {
      // ponytail: Align near-equal slot ends and fill the band from the left.
      final limit = bottoms.reduce(math.min) + _slotTolerance;
      final band = [
        for (var column = 0; column < _columns; column++)
          if (bottoms[column] <= limit) column,
      ];
      final top = band.map((column) => bottoms[column]).reduce(math.max);
      for (final column in band) {
        bottoms[column] = top;
      }
      final column = band.first;
      bottoms[column] = top + place(child, column, top);
    }
    if (_columns == 1) return bottoms;
    final columnsByBottom = List<int>.generate(_columns, (column) => column)
      ..sort((a, b) => bottoms[b].compareTo(bottoms[a]));
    var previousBottom = bottoms[columnsByBottom.first];
    var groupMax = previousBottom;
    for (final column in columnsByBottom.skip(1)) {
      final bottom = bottoms[column];
      if (previousBottom - bottom > _slotTolerance) groupMax = bottom;
      bottoms[column] = groupMax;
      previousBottom = bottom;
    }
    return bottoms;
  }

  @override
  void performLayout() {
    _columnWidth = _widthFor(constraints);
    final childConstraints = BoxConstraints.tightFor(width: _columnWidth);
    _bottoms = _stack((child, column, top) {
      child.layout(childConstraints, parentUsesSize: true);
      (child.parentData! as _MasonryParentData)
        ..column = column
        ..offset = Offset(column * _columnWidth, top);
      return child.size.height;
    });
    size = constraints.constrain(
      Size(_columnWidth * _columns, _bottoms.reduce(math.max)),
    );
  }

  @override
  Size computeDryLayout(BoxConstraints constraints) {
    final width = _widthFor(constraints);
    final childConstraints = BoxConstraints.tightFor(width: width);
    final bottoms = _stack(
      (child, _, _) => child.getDryLayout(childConstraints).height,
    );
    return constraints.constrain(
      Size(width * _columns, bottoms.reduce(math.max)),
    );
  }

  @override
  double computeMinIntrinsicWidth(double height) =>
      _columnSizing.minIntrinsicWidth(_cells, double.infinity);

  @override
  double computeMaxIntrinsicWidth(double height) =>
      _columnSizing.maxIntrinsicWidth(_cells, double.infinity);

  @override
  double computeMinIntrinsicHeight(double width) =>
      computeMaxIntrinsicHeight(width);

  @override
  double computeMaxIntrinsicHeight(double width) {
    final columnWidth = _widthFor(BoxConstraints.tightForFinite(width: width));
    return _stack(
      (child, _, _) => child.getMaxIntrinsicHeight(columnWidth),
    ).reduce(math.max);
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      defaultHitTestChildren(result, position: position);

  bool _isNestedGrid(RenderBox? child) => switch (child) {
    _RenderHairlineMasonry() => true,
    RenderAbstractLayoutBuilderMixin<BoxConstraints, RenderBox>(:final child) =>
      _isNestedGrid(child),
    _ => false,
  };

  @override
  void paint(PaintingContext context, Offset offset) {
    defaultPaint(context, offset);
    final canvas = context.canvas;
    if (_columns == 1) {
      // TableBorder centers inside strokes and insets the final stroke.
      TableBorder(
        horizontalInside: _border,
        bottom: _isNestedGrid(lastChild) ? BorderSide.none : _border,
      ).paint(
        canvas,
        offset & Size(_columnWidth, _bottoms.single),
        columns: const [],
        rows: [
          for (final child in _cells)
            if (child != lastChild && !_isNestedGrid(child))
              (child.parentData! as _MasonryParentData).offset.dy +
                  child.size.height,
        ],
      );
      return;
    }

    final paint = _border.toPaint();
    for (var column = 1; column < _columns; column++) {
      final x = column * _columnWidth;
      canvas.drawLine(
        offset + Offset(x, 0),
        offset + Offset(x, math.max(_bottoms[column - 1], _bottoms[column])),
        paint,
      );
    }
    // Each slot ends at the next cell's top or the final column bottom.
    final slotEnds = _bottoms.toList();
    for (var child = lastChild; child != null; child = childBefore(child)) {
      final data = child.parentData! as _MasonryParentData;
      final slotEnd = slotEnds[data.column];
      slotEnds[data.column] = data.offset.dy;
      if (_isNestedGrid(child)) continue;
      final start = offset + Offset(data.offset.dx, slotEnd);
      canvas.drawLine(start, start + Offset(_columnWidth, 0), paint);
    }
  }
}

class HairlineGridCell extends StatelessWidget {
  const HairlineGridCell({
    super.key,
    required this.child,
    this.padding,
    this.color,
  });
  final Widget child;
  final EdgeInsetsGeometry? padding;
  final Color? color;

  @override
  Widget build(BuildContext context) => ColoredBox(
    color: color ?? Colors.transparent,
    child: Padding(
      padding:
          padding ??
          (MediaQuery.sizeOf(context).width >= 720
              ? const EdgeInsets.symmetric(horizontal: 24, vertical: 20)
              : const EdgeInsets.symmetric(vertical: 12)),
      child: child,
    ),
  );
}

class StatusPill extends StatelessWidget {
  const StatusPill(this.label, {super.key, this.color});
  final String label;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final desktop = MediaQuery.sizeOf(context).width >= 720;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.controlSurface,
        borderRadius: BorderRadius.circular(999),
      ),
      child: Padding(
        padding: EdgeInsets.symmetric(
          horizontal: desktop ? 8 : 10,
          vertical: 4,
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          spacing: 6,
          children: [
            DecoratedBox(
              decoration: BoxDecoration(
                color: color ?? palette.success,
                shape: BoxShape.circle,
              ),
              child: const SizedBox.square(dimension: 6),
            ),
            Flexible(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: context.theme.typography.body.sm.copyWith(
                  color: palette.ink,
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class AppBadge extends StatelessWidget {
  const AppBadge(this.label, {super.key, this.background, this.foreground});
  final String label;
  final Color? background;
  final Color? foreground;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final desktop = MediaQuery.sizeOf(context).width >= 720;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: background ?? palette.successBackground,
        borderRadius: BorderRadius.circular(999),
      ),
      child: Padding(
        padding: EdgeInsets.symmetric(
          horizontal: desktop ? 10 : 9,
          vertical: desktop ? 2 : 1,
        ),
        child: Text(
          label,
          style: context.theme.typography.body.sm.copyWith(
            color: foreground ?? palette.successForeground,
            fontSize: desktop ? 13 : 12,
            fontWeight: FontWeight.w600,
          ),
        ),
      ),
    );
  }
}

class Section extends StatelessWidget {
  const Section({
    super.key,
    required this.title,
    required this.children,
    this.icon,
    this.trailing,
  });
  final String title;
  final List<Widget> children;
  final IconData? icon;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Padding(
        padding: const EdgeInsets.only(bottom: 8),
        child: Wrap(
          alignment: WrapAlignment.spaceBetween,
          crossAxisAlignment: WrapCrossAlignment.center,
          spacing: 8,
          runSpacing: 6,
          children: [
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (icon != null) ...[
                  Icon(
                    icon,
                    size: context.platformVariant.touch ? 18 : 16,
                    color: context.theme.colors.mutedForeground,
                  ),
                  const SizedBox(width: 8),
                ],
                EyebrowLabel(title),
              ],
            ),
            ?trailing,
          ],
        ),
      ),
      if (children.isNotEmpty)
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (final child in children)
              DecoratedBox(
                key: child.key == null
                    ? null
                    : ValueKey(('section-row', child.key)),
                decoration: BoxDecoration(
                  border: Border(
                    top: BorderSide(color: AppPalette.of(context).hairline),
                  ),
                ),
                child: Padding(
                  padding: EdgeInsets.symmetric(
                    vertical: context.platformVariant.touch ? 12 : 9,
                  ),
                  child: DefaultTextStyle.merge(
                    overflow: TextOverflow.visible,
                    child: child,
                  ),
                ),
              ),
          ],
        ),
    ],
  );
}

class SettingRow extends StatelessWidget {
  const SettingRow(
    this.label, {
    super.key,
    required this.child,
    this.description,
    this.stacked = false,
    this.keepInline = false,
    this.value,
    this.controlMaxWidth,
    this.minimumControlWidth = 0,
    this.controlAlignment = Alignment.centerRight,
  });
  final String label;
  final Widget child;
  final String? description;
  final bool stacked;
  final bool keepInline;
  final Widget? value;
  final double? controlMaxWidth;
  final double minimumControlWidth;
  final Alignment controlAlignment;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final title = Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            label,
            style: context.theme.typography.body.sm.copyWith(
              color: context.theme.colors.foreground,
              fontWeight: FontWeight.w400,
            ),
          ),
          if (description != null) ...[
            const SizedBox(height: 8),
            Text(
              description!,
              style: context.theme.typography.body.xs.copyWith(
                color: context.theme.colors.mutedForeground,
                height: 1.5,
              ),
            ),
          ],
        ],
      );
      final inline =
          !stacked &&
          constraints.maxWidth >= 480 &&
          constraints.maxWidth - 200 >= minimumControlWidth;
      return ConstrainedBox(
        constraints: const BoxConstraints(minHeight: 20),
        child: keepInline && !inline
            ? Row(
                children: [
                  Expanded(child: title),
                  child,
                ],
              )
            : inline
            ? Row(
                children: [
                  SizedBox(width: 200, child: title),
                  Expanded(
                    child: Align(
                      alignment: controlAlignment,
                      child: ConstrainedBox(
                        constraints: BoxConstraints(
                          maxWidth: controlMaxWidth ?? double.infinity,
                        ),
                        child: child,
                      ),
                    ),
                  ),
                  if (value != null) ...[const SizedBox(width: 8), value!],
                ],
              )
            : Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      Expanded(child: title),
                      ?value,
                    ],
                  ),
                  const SizedBox(height: 8),
                  child,
                ],
              ),
      );
    },
  );
}

class ChoiceSetting<T> extends StatelessWidget {
  const ChoiceSetting({
    super.key,
    required this.label,
    required this.value,
    required this.options,
    this.onChanged,
    this.disabled = const {},
    this.hint,
    this.scrollable = false,
  });
  final String label;
  final T value;
  final Map<T, String> options;
  final Set<T> disabled;
  final ValueChanged<T>? onChanged;
  final String? hint;
  final bool scrollable;

  @override
  Widget build(BuildContext context) {
    final colors = context.theme.colors;
    final accent = ChromeAccent.of(context);
    var optionWidth = 64.0;
    for (final option in options.values) {
      final painter = TextPainter(
        text: TextSpan(text: option, style: context.theme.typography.body.sm),
        textDirection: Directionality.of(context),
        textScaler: MediaQuery.textScalerOf(context),
      )..layout();
      if (painter.width + 32 > optionWidth) optionWidth = painter.width + 32;
      painter.dispose();
    }
    final minimumWidth = options.length * optionWidth + 8;
    return SettingRow(
      label,
      description: hint,
      stacked: minimumWidth > 360,
      controlMaxWidth: 360,
      minimumControlWidth: minimumWidth,
      child: LayoutBuilder(
        builder: (context, constraints) {
          if (disabled.isNotEmpty ||
              (!scrollable && minimumWidth > constraints.maxWidth)) {
            return FSelectGroup<T>(
              enabled: onChanged != null,
              control: FMultiValueControl.lifted(
                value: {value},
                onChange: (values) {
                  if (values.isNotEmpty) onChanged?.call(values.last);
                },
              ),
              children: [
                for (final entry in options.entries)
                  FSelectGroupItemMixin.radio(
                    value: entry.key,
                    label: Text(entry.value),
                    enabled: onChanged != null && !disabled.contains(entry.key),
                  ),
              ],
            );
          }
          final tabs = Semantics(
            enabled: onChanged != null,
            child: ExcludeFocus(
              excluding: onChanged == null,
              child: IgnorePointer(
                ignoring: onChanged == null,
                child: FTabs(
                  key: ValueKey((label, options.length)),
                  control: FTabControl.lifted(
                    index: options.keys
                        .toList()
                        .indexOf(value)
                        .clamp(0, options.length - 1),
                    onChange: (index) =>
                        onChanged?.call(options.keys.elementAt(index)),
                    motion: FTabMotion(
                      duration: MediaQuery.disableAnimationsOf(context)
                          ? Duration.zero
                          : const Duration(milliseconds: 150),
                      curve: Curves.easeOutCubic,
                    ),
                  ),
                  style: FTabsStyleDelta.delta(
                    spacing: 0,
                    padding: const EdgeInsetsGeometryDelta.value(
                      EdgeInsets.zero,
                    ),
                    // TabBar adds 2 px for its indicator below each tab.
                    minHeight: context.platformVariant.touch ? 38 : 32,
                    labelTextStyle: FVariants(
                      context.theme.typography.body.sm.copyWith(
                        color: onChanged == null
                            ? colors.mutedForeground
                            : colors.foreground,
                        fontWeight: FontWeight.w400,
                      ),
                      variants: const {},
                    ),
                    indicatorDecoration: DecorationDelta.value(
                      BoxDecoration(
                        color: accent.withValues(alpha: .12),
                        border: Border.all(color: accent),
                        borderRadius: BorderRadius.circular(8),
                      ),
                    ),
                  ),
                  children: [
                    for (final entry in options.entries)
                      FTabEntry(
                        label: Text(entry.value),
                        child: const SizedBox.shrink(),
                      ),
                  ],
                ),
              ),
            ),
          );
          return scrollable && minimumWidth > constraints.maxWidth
              ? SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  child: SizedBox(width: minimumWidth, child: tabs),
                )
              : tabs;
        },
      ),
    );
  }
}

class SwitchSetting extends StatelessWidget {
  const SwitchSetting(
    this.label, {
    super.key,
    required this.value,
    this.onChanged,
    this.hint,
  });
  final String label;
  final bool value;
  final ValueChanged<bool>? onChanged;
  final String? hint;

  @override
  Widget build(BuildContext context) {
    final colors = context.theme.colors;
    final accent = ChromeAccent.of(context);
    return SettingRow(
      label,
      description: hint,
      keepInline: true,
      child: FSwitch(
        semanticsLabel: AppStrings.toggle(label),
        value: value,
        enabled: onChanged != null,
        onChange: onChanged,
        style: FSwitchStyleDelta.delta(
          trackColor: FVariants(
            colors.secondary,
            variants: {
              [FSwitchVariant.selected]: accent,
              [FSwitchVariant.disabled]: colors.disable(colors.secondary),
              [FSwitchVariant.selected.and(FSwitchVariant.disabled)]: colors
                  .disable(accent),
            },
          ),
        ),
      ),
    );
  }
}

class SliderSetting extends StatefulWidget {
  const SliderSetting({
    super.key,
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.divisions,
    this.unit = '',
    this.valueText,
    this.onChanged,
    this.onPreview,
    this.semanticLabel,
    this.valueAction,
  });
  final String label;
  final double value;
  final double min;
  final double max;
  final int divisions;
  final String unit;
  final String? valueText;
  final ValueChanged<double>? onChanged;
  final ValueChanged<double>? onPreview;
  final String? semanticLabel;
  final Widget? valueAction;
  @override
  State<SliderSetting> createState() => _SliderSettingState();
}

class _SliderSettingState extends State<SliderSetting> {
  double? _draft;
  double _value(double fraction) =>
      widget.min +
      (fraction * widget.divisions).round() /
          widget.divisions *
          (widget.max - widget.min);
  String _label(double value) => '${value.round()}${widget.unit}';
  @override
  void didUpdateWidget(covariant SliderSetting oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.value != widget.value || widget.onChanged == null) {
      _draft = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    final value = (_draft ?? widget.value).clamp(widget.min, widget.max);
    final accent = ChromeAccent.of(context);
    final displayedLabel = _draft == null && widget.valueText != null
        ? widget.valueText!
        : _label(value);
    final valueLabel = Text(
      displayedLabel,
      textAlign: TextAlign.right,
      style: context.theme.typography.body.xs.copyWith(
        color: context.theme.colors.mutedForeground,
        fontFeatures: const [FontFeature.tabularFigures()],
      ),
    );
    return SettingRow(
      widget.label,
      value: context.platformVariant.touch
          ? Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                SizedBox(width: 56, child: valueLabel),
                ?widget.valueAction,
              ],
            )
          : ConstrainedBox(
              constraints: const BoxConstraints(minWidth: 56, minHeight: 24),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                mainAxisAlignment: MainAxisAlignment.end,
                spacing: 4,
                children: [?widget.valueAction, valueLabel],
              ),
            ),
      child: FSlider(
        enabled: widget.onChanged != null,
        style: FSliderStyleDelta.delta(
          activeColor: FVariants(
            accent,
            variants: {
              [FSliderVariant.disabled]: context.theme.colors.disable(accent),
            },
          ),
        ),
        control: FSliderControl.liftedContinuous(
          value: FSliderValue(
            max: (value - widget.min) / (widget.max - widget.min),
          ),
          stepPercentage: 1 / widget.divisions,
          onChange: (value) {
            final draft = _value(value.max);
            setState(() => _draft = draft);
            widget.onPreview?.call(draft);
          },
        ),
        tooltipBuilder: (_, value) => Text(_label(_value(value))),
        semanticValueFormatterCallback: (value) =>
            '${widget.semanticLabel ?? widget.label} ${_label(_value(value))}',
        onEnd: widget.onChanged == null
            ? null
            : (value) {
                widget.onChanged!(_draft ?? _value(value.max));
                if (mounted) setState(() => _draft = null);
              },
      ),
    );
  }
}

class ActionButton extends StatelessWidget {
  const ActionButton(
    this.label, {
    super.key,
    this.onPress,
    this.icon,
    this.primary = false,
    this.success = false,
    this.compact = false,
    this.tooltip,
  });
  final String label;
  final VoidCallback? onPress;
  final IconData? icon;
  final bool primary;
  final bool success;
  final bool compact;
  final String? tooltip;
  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final colors = context.theme.colors;
    final background = success
        ? palette.successBackground
        : primary
        ? ChromeAccent.of(context)
        : Colors.transparent;
    final foreground = success
        ? palette.successForeground
        : primary
        ? ChromeAccent.foreground(background)
        : palette.ink;
    ShapeDecoration decoration(Color fill, Color stroke) => ShapeDecoration(
      color: fill,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(999),
        side: primary ? BorderSide.none : BorderSide(color: stroke, width: 1.5),
      ),
    );
    final button = FButton(
      onPress: onPress,
      variant: primary ? FButtonVariant.primary : FButtonVariant.outline,
      mainAxisSize: MainAxisSize.min,
      style: FButtonStyleDelta.delta(
        decoration: FVariants.from(
          decoration(background, palette.ink),
          variants: {
            [
              FTappableVariant.hovered,
              FTappableVariant.pressed,
            ]: DecorationDelta.value(
              decoration(
                primary ? colors.hover(background) : palette.controlSurface,
                palette.ink,
              ),
            ),
            [FTappableVariant.disabled]: DecorationDelta.value(
              decoration(
                success
                    ? background
                    : primary
                    ? colors.disable(background)
                    : Colors.transparent,
                success ? palette.success : colors.disable(palette.ink),
              ),
            ),
          },
        ),
        focusedOutlineStyle: FFocusedOutlineStyleDelta.delta(
          color: ChromeAccent.of(context),
          borderRadius: BorderRadius.circular(999),
        ),
        contentStyle: FButtonContentStyleDelta.delta(
          constraints: const BoxConstraints(minWidth: 44, minHeight: 44),
          padding: EdgeInsetsGeometryDelta.value(
            EdgeInsets.symmetric(horizontal: compact ? 8 : 15, vertical: 8),
          ),
          textStyle: FVariants.from(
            context.theme.typography.body.sm.copyWith(
              color: foreground,
              fontWeight: FontWeight.w600,
              height: 1.2,
            ),
            variants: {
              [FTappableVariant.disabled]: TextStyleDelta.delta(
                color: success ? foreground : colors.disable(foreground),
              ),
            },
          ),
          iconStyle: FVariants.from(
            IconThemeData(color: foreground),
            variants: {
              [FTappableVariant.disabled]: IconThemeDataDelta.delta(
                color: success ? foreground : colors.disable(foreground),
              ),
            },
          ),
        ),
      ),
      prefix: icon == null
          ? null
          : Icon(icon, size: context.platformVariant.touch ? 18 : 16),
      child: Flexible(child: Text(label)),
    );
    return tooltip == null
        ? button
        : FTooltip(tipBuilder: (_, _) => Text(tooltip!), child: button);
  }
}

class ActionRow extends StatelessWidget {
  const ActionRow({
    super.key,
    required this.children,
    this.keepPairTogether = false,
  });
  final List<Widget> children;
  final bool keepPairTogether;
  @override
  Widget build(BuildContext context) => keepPairTogether && children.length == 2
      ? Row(
          spacing: 6,
          children: [
            Flexible(child: children.first),
            children.last,
          ],
        )
      : Wrap(
          spacing: 8,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: children,
        );
}

class ScrollChrome extends StatelessWidget {
  const ScrollChrome({
    super.key,
    required this.fraction,
    required this.alignment,
    required this.child,
    this.visible = true,
  });

  final ValueListenable<double>? fraction;
  final Alignment alignment;
  final Widget child;
  final bool visible;

  Widget _clip(double height, Widget child) => ExcludeSemantics(
    excluding: height == 0,
    child: ExcludeFocus(
      excluding: height == 0,
      child: IgnorePointer(
        ignoring: height == 0,
        child: ClipRect(
          child: Align(
            alignment: alignment,
            heightFactor: height,
            child: child,
          ),
        ),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) {
    if (!visible) return const SizedBox.shrink();
    final progress = fraction;
    if (progress == null) return child;
    return ValueListenableBuilder<double>(
      valueListenable: progress,
      child: child,
      builder: (_, height, child) => _clip(height, child!),
    );
  }
}

class ValueRow extends StatelessWidget {
  const ValueRow(this.label, this.value, {super.key, this.trailing});
  final String label;
  final String value;
  final Widget? trailing;
  @override
  Widget build(BuildContext context) => SettingRow(
    label,
    child: Row(
      children: [
        Expanded(
          child: SelectableText(
            value,
            style: context.theme.typography.body.xs.copyWith(
              color: context.theme.colors.mutedForeground,
            ),
          ),
        ),
        if (trailing != null) ...[const SizedBox(width: 8), trailing!],
      ],
    ),
  );
}

class AppPageBody extends StatelessWidget {
  const AppPageBody({
    super.key,
    required this.controller,
    required this.page,
    required this.children,
    this.fullWidth = false,
    this.spacing = 24,
  });
  final AppController controller;
  final AppPage page;
  final List<Widget> children;
  final bool fullWidth;
  final double spacing;
  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    key: PageStorageKey(page.id),
    primary: false,
    padding: fullWidth
        ? MediaQuery.sizeOf(context).width >= 720
              ? EdgeInsets.zero
              : const EdgeInsets.fromLTRB(16, 0, 16, 24)
        : MediaQuery.sizeOf(context).width >= 720
        ? const EdgeInsets.fromLTRB(24, 6, 24, 24)
        : const EdgeInsets.fromLTRB(16, 8, 16, 24),
    child: Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: fullWidth ? 1440 : 760),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: spacing,
          children: [
            if (controller.appUpdate?.hasUpdate == true)
              _notice(AppUpdateBanner(controller: controller)),
            for (final update in controller.visibleSchemeUpdates)
              _notice(
                SchemeUpdateBanner(controller: controller, update: update),
              ),
            if (controller.info.platform != BridgePlatform.macOs &&
                controller.unhandledError != null)
              _notice(ErrorMessage(controller.unhandledError)),
            ...children,
          ],
        ),
      ),
    ),
  );

  Widget _notice(Widget child) =>
      fullWidth ? HairlineGridCell(child: child) : child;
}

class AppUpdateBanner extends StatelessWidget {
  const AppUpdateBanner({super.key, required this.controller, this.onDownload});
  final AppController controller;
  final ValueChanged<bool>? onDownload;

  void _download({bool redownload = false}) {
    if (onDownload case final confirm?) {
      confirm(redownload);
    } else {
      controller.openAppUpdate(redownload: redownload);
    }
  }

  @override
  Widget build(BuildContext context) {
    final update = controller.appUpdate;
    if (update == null || !update.hasUpdate) return const SizedBox.shrink();
    final locale = Localizations.localeOf(context);
    final action = controller.appUpdateAction;
    final (label, icon) = switch (action) {
      AppUpdateAction.download => (
        AppStrings.downloadAppUpdatePackage(locale),
        FLucideIcons.download,
      ),
      AppUpdateAction.openPackage => (
        AppStrings.openAppUpdatePackage(locale),
        FLucideIcons.packageOpen,
      ),
      AppUpdateAction.restart => (
        AppStrings.restartForAppUpdate(locale),
        FLucideIcons.refreshCw,
      ),
      AppUpdateAction.openLink => (
        AppStrings.downloadAppUpdate(locale),
        FLucideIcons.externalLink,
      ),
    };
    final progress = controller.appUpdateProgress;
    final percent = progress?.percent.clamp(0, 100);
    final canRunAction = action == AppUpdateAction.download
        ? controller.canDownloadAppUpdate
        : controller.canRunAppUpdate;
    return FTileGroup(
      children: [
        FTile(
          prefix: Icon(
            FLucideIcons.arrowUpCircle,
            size: context.platformVariant.touch ? 18 : 16,
          ),
          title: Text(
            AppStrings.appUpdate,
            style: context.theme.typography.body.sm.copyWith(
              fontWeight: FontWeight.w500,
            ),
          ),
          subtitle: Wrap(
            spacing: 8,
            runSpacing: 4,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              FBadge(child: Text('v${update.latestVersion}')),
              Text(AppStrings.currentVersion(update.currentVersion)),
              Text(label),
            ],
          ),
          suffix: Icon(icon, size: 16),
          onPress: canRunAction
              ? action == AppUpdateAction.download
                    ? _download
                    : controller.openAppUpdate
              : null,
        ),
        if (action == AppUpdateAction.restart)
          FTile(
            title: Text(AppStrings.openAppUpdatePackage(locale)),
            suffix: const Icon(FLucideIcons.packageOpen, size: 16),
            onPress: controller.canRunAppUpdate
                ? () => controller.openAppUpdate(interactive: true)
                : null,
          ),
        if (controller.appUpdateBusy)
          FTile.raw(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 8,
              children: [
                if (percent == null)
                  const FProgress()
                else
                  FDeterminateProgress(value: percent / 100),
                Semantics(
                  liveRegion: true,
                  child: Text(
                    progress != null
                        ? '${progress.message} $percent%'
                        : action == AppUpdateAction.download
                        ? AppStrings.appUpdateDownloading(locale)
                        : action == AppUpdateAction.restart
                        ? AppStrings.appUpdateInstalling(locale)
                        : AppStrings.appUpdateOpening(locale),
                  ),
                ),
              ],
            ),
          ),
        if (controller.appUpdateActionError case final error?) ...[
          FTile.raw(child: ErrorMessage(error)),
          if (action == AppUpdateAction.restart ||
              action == AppUpdateAction.openPackage)
            FTile(
              title: Text(AppStrings.redownloadAppUpdatePackage(locale)),
              suffix: const Icon(FLucideIcons.download, size: 16),
              onPress: controller.canDownloadAppUpdate
                  ? () => _download(redownload: true)
                  : null,
            ),
        ],
      ],
    );
  }
}

class SchemeUpdateBanner extends StatelessWidget {
  const SchemeUpdateBanner({
    super.key,
    required this.controller,
    required this.update,
  });
  final AppController controller;
  final SchemeUpdateDto update;

  @override
  Widget build(BuildContext context) => FTileGroup(
    children: [
      FTile.raw(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Icon(
                  FLucideIcons.arrowUpCircle,
                  size: context.platformVariant.touch ? 18 : 16,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    controller.packageDisplayName(update.schemeKey),
                    style: context.theme.typography.body.sm.copyWith(
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ),
                FTooltip(
                  tipBuilder: (_, _) => const Text(AppStrings.close),
                  child: FButton.icon(
                    variant: FButtonVariant.ghost,
                    onPress: controller.busy
                        ? null
                        : () => controller.dismissSchemeUpdate(update),
                    child: const Icon(
                      FLucideIcons.x,
                      size: 16,
                      semanticLabel: AppStrings.close,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            ActionRow(
              children: [
                Text(
                  '→ ${update.latestVersion}',
                  style: context.theme.typography.body.sm.copyWith(
                    color: context.theme.colors.mutedForeground,
                  ),
                ),
                ActionButton(
                  '更新',
                  primary: true,
                  onPress: controller.busy
                      ? null
                      : () => controller.updateSchemePackage(update.schemeKey),
                ),
              ],
            ),
          ],
        ),
      ),
    ],
  );
}

class AppBrand extends StatelessWidget {
  const AppBrand({super.key, required this.version, this.compact = false});
  final String version;
  final bool compact;
  @override
  Widget build(BuildContext context) => Row(
    children: [
      Image.asset(
        logoAsset,
        width: compact ? 28 : 48,
        height: compact ? 28 : 48,
        semanticLabel: 'KeyTao',
      ),
      const SizedBox(width: 12),
      Expanded(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              AppStrings.title,
              style: context.theme.typography.body.sm.copyWith(
                fontWeight: FontWeight.w500,
              ),
            ),
            Text(
              'v$version',
              style: context.theme.typography.body.xs.copyWith(
                color: context.theme.colors.mutedForeground,
              ),
            ),
            if (!compact) ...[
              const SizedBox(height: 8),
              Text(AppStrings.tagline, style: context.theme.typography.body.xs),
            ],
          ],
        ),
      ),
    ],
  );
}

Color successColor(BuildContext context) => AppPalette.of(context).success;
Color warningColor(BuildContext context) =>
    context.theme.colors.brightness == Brightness.dark
    ? const Color(0xFFFCD34D)
    : const Color(0xFF92400E);

class StatusMessage extends StatelessWidget {
  const StatusMessage(
    this.message, {
    super.key,
    this.icon = FLucideIcons.info,
    this.color,
  });
  final String message;
  final IconData icon;
  final Color? color;
  @override
  Widget build(BuildContext context) => Semantics(
    liveRegion: true,
    child: Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            icon,
            size: context.platformVariant.touch ? 18 : 16,
            color: color ?? context.theme.colors.mutedForeground,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: SelectableText(
              message,
              style: context.theme.typography.body.sm.copyWith(
                color: color ?? context.theme.colors.mutedForeground,
                height: 1.5,
              ),
            ),
          ),
        ],
      ),
    ),
  );
}

class ErrorMessage extends StatelessWidget {
  const ErrorMessage(this.message, {super.key});
  final String? message;
  @override
  Widget build(BuildContext context) => message?.isNotEmpty == true
      ? StatusMessage(
          message!,
          icon: FLucideIcons.circleAlert,
          color: context.theme.colors.destructive,
        )
      : const SizedBox.shrink();
}

class LogLines extends StatefulWidget {
  const LogLines({
    super.key,
    required this.lines,
    required this.storageKey,
    this.height = 240,
    this.colorCoded = false,
  });
  final List<String> lines;
  final String storageKey;
  final double height;
  final bool colorCoded;
  @override
  State<LogLines> createState() => _LogLinesState();
}

class _LogLinesState extends State<LogLines> {
  final _scrollController = ScrollController();
  @override
  void dispose() {
    _scrollController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final scrollBehavior = ScrollConfiguration.of(context)
        .copyWith(scrollbars: false);
    return SizedBox(
      height: widget.height,
      child: widget.lines.isEmpty
          ? const Center(child: Text(AppStrings.noLogs))
          : Scrollbar(
              controller: _scrollController,
              child: ScrollConfiguration(
                behavior: scrollBehavior,
                child: ListView.builder(
                  key: PageStorageKey(widget.storageKey),
                  controller: _scrollController,
                  primary: false,
                  itemCount: widget.lines.length,
                  itemBuilder: (context, index) => Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: SelectableText(
                      widget.lines[index],
                      scrollBehavior: scrollBehavior,
                      style: context.theme.typography.body.xs.copyWith(
                        fontFamily: 'monospace',
                        color: !widget.colorCoded
                            ? null
                            : switch (operationLogKind(widget.lines[index])) {
                                LogLineKind.error =>
                                  context.theme.colors.destructive,
                                LogLineKind.warning => warningColor(context),
                                LogLineKind.deploy => successColor(context),
                                LogLineKind.merged =>
                                  context.theme.colors.primary,
                                LogLineKind.normal =>
                                  context.theme.colors.foreground,
                              },
                      ),
                    ),
                  ),
                ),
              ),
            ),
    );
  }
}

class AppDialog extends StatelessWidget {
  const AppDialog({
    super.key,
    required this.title,
    required this.child,
    required this.actions,
    required this.animation,
    this.scrollable = true,
    this.constraints = const BoxConstraints(minWidth: 280, maxWidth: 560),
  });
  final String title;
  final Widget child;
  final List<Widget> actions;
  final Animation<double> animation;
  final bool scrollable;
  final BoxConstraints constraints;
  @override
  Widget build(BuildContext context) => FDialog(
    animation: animation,
    constraints: constraints,
    builder: (context, _) => Padding(
      padding: EdgeInsets.symmetric(
        horizontal: MediaQuery.sizeOf(context).width >= 720 ? 24 : 16,
        vertical: 18,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            title,
            style: context.theme.typography.display.lg.copyWith(
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: 12),
          Flexible(
            child: scrollable ? SingleChildScrollView(child: child) : child,
          ),
          const SizedBox(height: 16),
          Align(
            alignment: Alignment.centerRight,
            child: ActionRow(children: actions),
          ),
        ],
      ),
    ),
  );
}
