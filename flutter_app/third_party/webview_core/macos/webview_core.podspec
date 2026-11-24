Pod::Spec.new do |s|
  s.name             = 'webview_core'
  s.version          = '1.1.0'
  s.summary          = 'Zero-dependency native WebView plugin for Flutter.'
  s.description      = <<-DESC
Native WebView plugin for Flutter across Android, iOS, Linux, macOS, Windows, and Web with zero external hosted dependencies.
                       DESC
  s.homepage         = 'https://github.com/infradise/webview_core'
  s.license          = { :type => 'Apache 2.0', :file => '../LICENSE' }
  s.author           = { 'Infradise Inc.' => 'opensource@infradise.com' }
  s.source           = { :path => '.' }
  s.source_files     = 'webview_core/Sources/webview_core/**/*.swift'
  s.resource_bundles = {'webview_core_privacy' => ['webview_core/Sources/webview_core/Resources/PrivacyInfo.xcprivacy']}
  s.dependency 'FlutterMacOS'
  s.frameworks       = 'AppKit', 'WebKit'

  s.platform = :osx, '11.0'
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES' }
  s.swift_version = '5.0'
end
