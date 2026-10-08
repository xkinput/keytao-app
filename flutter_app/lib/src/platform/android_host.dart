import 'dart:async';

import 'package:flutter/services.dart';

import '../rust/api/types.dart' show InstallProgressDto;

class AndroidHost {
  const AndroidHost();
  static const _channel = MethodChannel('keytao/android');
  static final StreamController<InstallProgressDto> _progress =
      StreamController<InstallProgressDto>.broadcast(
        onListen: () => _channel.setMethodCallHandler((call) async {
          if (call.method != 'installProgress') return;
          final value = Map<String, dynamic>.from(call.arguments as Map);
          _progress.add(
            InstallProgressDto(
              stage: value['stage'] as String,
              percent: (value['percent'] as num).toInt(),
              message: value['message'] as String,
            ),
          );
        }),
        onCancel: () => _channel.setMethodCallHandler(null),
      );

  Stream<InstallProgressDto> get installProgress => _progress.stream;

  Future<Map<String, dynamic>> _map(
    String method, [
    Map<String, Object?>? arguments,
  ]) async {
    final result = await _channel.invokeMapMethod<String, dynamic>(
      method,
      arguments,
    );
    if (result == null) {
      throw PlatformException(code: 'keytao', message: '系统未返回操作结果');
    }
    return result;
  }

  Future<Map<String, dynamic>> paths() => _map('paths');
  Future<Map<String, dynamic>> imeStatus() => _map('imeStatus');
  Future<Map<String, dynamic>> storagePermissionStatus() =>
      _map('storagePermissionStatus');
  Future<void> openStoragePermissionSettings() =>
      _channel.invokeMethod<void>('openStoragePermissionSettings');
  Future<void> openInputMethodSettings() =>
      _channel.invokeMethod<void>('openInputMethodSettings');
  Future<void> showInputMethodPicker() =>
      _channel.invokeMethod<void>('showInputMethodPicker');
  Future<String?> pickDirectory() async =>
      (await _channel.invokeMapMethod<String, dynamic>('pickDirectory'))?['uri']
          as String?;
  Future<Map<String, dynamic>> listFiles(String treeUri) =>
      _map('listFiles', {'treeUri': treeUri});
  Future<Map<String, dynamic>> readLocalSchemas(String treeUri) =>
      _map('readLocalSchemas', {'treeUri': treeUri});
  Future<Map<String, dynamic>> smartExtractZip(
    String zipPath,
    String treeUri,
  ) => _map('smartExtractZip', {'zipPath': zipPath, 'treeUri': treeUri});
  Future<Map<String, dynamic>> smartExtractZipToPrivate(String zipPath) =>
      _map('smartExtractZipToPrivate', {'zipPath': zipPath});
  Future<Map<String, dynamic>> copyAddonSchemaAssets(String id) =>
      _map('copyAddonSchemaAssets', {'id': id});
  Future<Map<String, dynamic>> deployImeData() => _map('deployImeData');
  Future<void> adoptAppLogger() =>
      _channel.invokeMethod<void>('adoptAppLogger');
  Future<void> openUrl(String url) =>
      _channel.invokeMethod<void>('openUrl', {'url': url});
  Future<Map<String, dynamic>?> shareRuntimeLog() =>
      _channel.invokeMapMethod<String, dynamic>('shareRuntimeLog');
}
