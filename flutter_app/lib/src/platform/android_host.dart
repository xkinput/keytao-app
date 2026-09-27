import 'package:flutter/services.dart';

class AndroidHost {
  const AndroidHost();
  static const _channel = MethodChannel('keytao/android');

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
  Future<Map<String, dynamic>> keytaoRoot() => _map('keytaoRoot');
  Future<Map<String, dynamic>> storagePermissionStatus() =>
      _map('storagePermissionStatus');
  Future<void> openStoragePermissionSettings() =>
      _channel.invokeMethod<void>('openStoragePermissionSettings');
  Future<void> openInputMethodSettings() =>
      _channel.invokeMethod<void>('openInputMethodSettings');
  Future<void> showInputMethodPicker() =>
      _channel.invokeMethod<void>('showInputMethodPicker');
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
