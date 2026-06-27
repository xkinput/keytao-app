import Foundation

enum KeyTaoIOSBundle {
    static var resourceBundles: [Bundle] {
        var bundles: [Bundle] = []
        #if SWIFT_PACKAGE
        bundles.append(.module)
        #endif
        bundles.append(contentsOf: [Bundle(for: BundleToken.self), .main])
        bundles.append(contentsOf: Bundle.allBundles)
        return bundles
    }

    static func url(forResource name: String, withExtension ext: String? = nil) -> URL? {
        for bundle in resourceBundles {
            if let url = bundle.url(forResource: name, withExtension: ext) {
                return url
            }
            if let url = bundle.url(forResource: name, withExtension: ext, subdirectory: "Resources") {
                return url
            }
        }
        return nil
    }
}

private final class BundleToken {}
