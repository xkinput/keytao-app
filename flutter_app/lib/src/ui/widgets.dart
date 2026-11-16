import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'theme.dart';

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
        child: Row(
          children: [
            if (icon != null) ...[
              Icon(
                icon,
                size: context.platformVariant.touch ? 18 : 16,
                color: context.theme.colors.mutedForeground,
              ),
              const SizedBox(width: 8),
            ],
            Expanded(
              child: Text(
                title,
                style: context.theme.typography.body.sm.copyWith(
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
            ?trailing,
          ],
        ),
      ),
      if (children.isNotEmpty)
        FTileGroup(
          style: FTileGroupStyleDelta.delta(
            decoration: DecorationDelta.value(
              ShapeDecoration(
                shape: RoundedRectangleBorder(
                  side: BorderSide(color: context.theme.colors.border),
                  borderRadius: BorderRadius.circular(12),
                ),
              ),
            ),
          ),
          divider: FItemDivider.full,
          children: [
            for (final child in children)
              FTile.raw(
                style: const FItemStyleDelta.delta(
                  padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
                  rawContentStyle: FRawItemContentStyleDelta.delta(
                    padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
                  ),
                ),
                child: Padding(
                  padding: EdgeInsets.symmetric(
                    horizontal: 16,
                    vertical: context.platformVariant.touch ? 16 : 12,
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
  });
  final String label;
  final T value;
  final Map<T, String> options;
  final Set<T> disabled;
  final ValueChanged<T>? onChanged;
  final String? hint;

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
          if (disabled.isNotEmpty || minimumWidth > constraints.maxWidth) {
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
          return Semantics(
            enabled: onChanged != null,
            child: ExcludeFocus(
              excluding: onChanged == null,
              child: IgnorePointer(
                ignoring: onChanged == null,
                child: FTabs(
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
    this.onChanged,
    this.onPreview,
    this.semanticLabel,
  });
  final String label;
  final double value;
  final double min;
  final double max;
  final int divisions;
  final String unit;
  final ValueChanged<double>? onChanged;
  final ValueChanged<double>? onPreview;
  final String? semanticLabel;
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
    return SettingRow(
      widget.label,
      value: SizedBox(
        width: 56,
        child: Text(
          _label(value),
          textAlign: TextAlign.right,
          style: context.theme.typography.body.xs.copyWith(
            color: context.theme.colors.mutedForeground,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
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
                widget.onChanged!(_value(value.max));
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
    this.tooltip,
  });
  final String label;
  final VoidCallback? onPress;
  final IconData? icon;
  final bool primary;
  final String? tooltip;
  @override
  Widget build(BuildContext context) {
    final button = FButton(
      onPress: onPress,
      variant: primary ? FButtonVariant.primary : FButtonVariant.outline,
      mainAxisSize: MainAxisSize.min,
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
  const ActionRow({super.key, required this.children});
  final List<Widget> children;
  @override
  Widget build(BuildContext context) => Wrap(
    spacing: 8,
    runSpacing: 8,
    crossAxisAlignment: WrapCrossAlignment.center,
    children: children,
  );
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
  });
  final AppController controller;
  final AppPage page;
  final List<Widget> children;
  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    key: PageStorageKey(page.id),
    primary: false,
    padding: context.platformVariant.touch
        ? const EdgeInsets.fromLTRB(16, 8, 16, 24)
        : const EdgeInsets.fromLTRB(32, 8, 32, 32),
    child: Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 760),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 32,
          children: [
            if (controller.appUpdate?.hasUpdate == true)
              AppUpdateBanner(controller: controller),
            for (final update in controller.visibleSchemeUpdates)
              SchemeUpdateBanner(controller: controller, update: update),
            if (controller.info.platform != BridgePlatform.macOs &&
                controller.unhandledError != null)
              ErrorMessage(controller.unhandledError),
            ...children,
          ],
        ),
      ),
    ),
  );
}

class AppUpdateBanner extends StatelessWidget {
  const AppUpdateBanner({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final update = controller.appUpdate!;
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
            ],
          ),
          suffix: const Icon(FLucideIcons.externalLink, size: 16),
          onPress: controller.openAppUpdate,
        ),
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
                    schemeDisplayName(update.schemeKey),
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

Color successColor(BuildContext context) =>
    context.theme.colors.brightness == Brightness.dark
    ? const Color(0xFF86EFAC)
    : const Color(0xFF166534);
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
  });
  final String title;
  final Widget child;
  final List<Widget> actions;
  final Animation<double> animation;
  final bool scrollable;
  @override
  Widget build(BuildContext context) => FDialog(
    animation: animation,
    builder: (context, _) => Padding(
      padding: const EdgeInsets.all(24),
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
          const SizedBox(height: 16),
          Flexible(
            child: scrollable ? SingleChildScrollView(child: child) : child,
          ),
          const SizedBox(height: 20),
          Align(
            alignment: Alignment.centerRight,
            child: ActionRow(children: actions),
          ),
        ],
      ),
    ),
  );
}
