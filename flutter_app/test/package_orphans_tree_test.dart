import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/rust/api/types.dart';
import 'package:keytao/src/ui/package_orphans.dart';

PackageOrphanFileDto orphan(String path, int size) =>
    PackageOrphanFileDto(path: path, sha256: 'unused', size: BigInt.from(size));

void main() {
  test('groups path segments and retains root files and original DTOs', () {
    final rootFile = orphan('root.yaml', 1);
    final direct = orphan('build/shared.bin', 2);
    final nested = orphan('build/子目录/shared.bin', 4);
    final sibling = orphan('build-extra/shared.bin', 8);
    final tree = buildPackageOrphanTree([nested, rootFile, direct, sibling]);
    final build = tree.directories['build']!;
    final child = build.directories['子目录']!;

    expect(tree.files, [same(rootFile)]);
    expect(tree.directories.keys, ['build', 'build-extra']);
    expect(build.path, 'build');
    expect(build.files, [same(direct)]);
    expect(child.path, 'build/子目录');
    expect(child.files, [same(nested)]);
    expect(build.descendantFiles, unorderedEquals([direct, nested]));
    expect(build.descendantFiles.length, 2);
    expect(build.totalSize, BigInt.from(6));
    expect(tree.totalSize, BigInt.from(15));
  });

  test('directory state reflects all descendant file paths', () {
    final tree = buildPackageOrphanTree([
      orphan('build/one.bin', 1),
      orphan('build/nested/two.bin', 2),
      orphan('other/one.bin', 4),
    ]);
    final build = tree.directories['build']!;
    final nested = build.directories['nested']!;
    final selected = <String>{'other/one.bin'};

    expect(build.selectionState(selected), isFalse);
    selected.add('build/nested/two.bin');
    expect(build.selectionState(selected), isNull);
    expect(nested.selectionState(selected), isTrue);
    selected.addAll(build.descendantFiles.map((file) => file.path));
    expect(build.selectionState(selected), isTrue);
    selected.removeAll(build.descendantFiles.map((file) => file.path));
    expect(build.selectionState(selected), isFalse);
    expect(selected, {'other/one.bin'});
  });

  test('an empty report has no directories or selected descendants', () {
    final tree = buildPackageOrphanTree([]);

    expect(tree.files, isEmpty);
    expect(tree.directories, isEmpty);
    expect(tree.totalSize, BigInt.zero);
    expect(tree.selectionState({'missing.yaml'}), isFalse);
  });
}
