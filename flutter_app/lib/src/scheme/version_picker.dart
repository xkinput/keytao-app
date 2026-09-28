import 'package:flutter/material.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../app/widgets.dart';

class SchemeSelection extends StatelessWidget {
  const SchemeSelection({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        LayoutBuilder(
          builder: (context, constraints) {
            final columns = constraints.maxWidth >= 560 ? 4 : 2;
            final width = (constraints.maxWidth - (columns - 1) * 8) / columns;
            return Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                for (final entry in schemes.entries)
                  SizedBox(
                    width: width,
                    child: Tooltip(
                      message: schemeAssets[entry.key]!,
                      child: ChoiceChip(
                        showCheckmark: false,
                        selected: c.scheme == entry.key,
                        label: SizedBox(
                          width: double.infinity,
                          child: Column(
                            children: [
                              Text(entry.value),
                              Text(
                                c.scheme == entry.key &&
                                        c.releaseVersion != null
                                    ? c.releaseVersion!
                                    : schemeAssets[entry.key]!,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: Theme.of(context).textTheme.labelSmall,
                              ),
                            ],
                          ),
                        ),
                        onSelected: c.busy || c.releaseLoading
                            ? null
                            : (_) => c.selectScheme(entry.key),
                      ),
                    ),
                  ),
              ],
            );
          },
        ),
        StatusMessage(c.schemeSelectionText, icon: Icons.download_outlined),
      ],
    );
  }
}

class VersionPicker extends StatelessWidget {
  const VersionPicker({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final sources = c.sourceVersions;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Wrap(
          spacing: 8,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            if (c.scheme == 'keytao' && sources.containsKey('github'))
              for (final source in sources.entries)
                ChoiceChip(
                  label: Text('${downloadSources[source.key]} ${source.value}'),
                  selected: c.source == source.key,
                  onSelected: c.busy || c.releaseLoading
                      ? null
                      : (_) => c.selectSource(source.key),
                ),
            if (c.scheme == 'keytao' &&
                sources.length == 1 &&
                !sources.containsKey('github'))
              Chip(
                label: Text(
                  '${downloadSources[sources.keys.single]} ${sources.values.single}',
                ),
              ),
            if (c.scheme == 'keytao' &&
                sources.isEmpty &&
                c.latestRelease != null)
              Chip(label: Text(c.latestRelease!.version)),
            if (c.scheme != 'keytao' && c.releaseVersion != null)
              Chip(
                label: Text('${c.releaseVersion} · ${c.selectedSchemeAsset}'),
              ),
            if (c.changelogBody != null)
              TextButton(
                onPressed: () => showDialog<void>(
                  context: context,
                  builder: (context) => AlertDialog(
                    title: Text(c.changelogTitle!),
                    content: SizedBox(
                      width: 560,
                      child: SingleChildScrollView(
                        child: SelectableText(
                          c.changelogBody!,
                          style: Theme.of(context).textTheme.bodySmall
                              ?.copyWith(fontFamily: 'monospace'),
                        ),
                      ),
                    ),
                    actions: [
                      TextButton(
                        onPressed: () => Navigator.pop(context),
                        child: const Text(AppStrings.close),
                      ),
                    ],
                  ),
                ),
                child: const Text(AppStrings.changelog),
              ),
          ],
        ),
        Row(
          children: [
            Expanded(child: Text('最新版本 ${c.releaseVersion ?? '—'}')),
            IconButton(
              tooltip: AppStrings.checkUpdate,
              onPressed: c.busy || c.releaseLoading ? null : c.refreshRelease,
              icon: const Icon(Icons.refresh),
            ),
          ],
        ),
        if (c.releaseLoading) const LinearProgressIndicator(),
        ErrorMessage(c.releaseError),
      ],
    );
  }
}
