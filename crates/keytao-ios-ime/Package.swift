// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "KeyTaoIOSIME",
    platforms: [.iOS(.v15)],
    products: [
        .library(name: "KeyTaoIOSIME", targets: ["KeyTaoIOSIME"]),
    ],
    targets: [
        .systemLibrary(
            name: "CKeytaoCore",
            path: "Sources/CKeytaoCore",
            pkgConfig: nil,
            providers: nil
        ),
        .target(
            name: "KeyTaoIOSIME",
            dependencies: ["CKeytaoCore"],
            path: "Sources/KeyTaoIOSIME",
            resources: [
                .process("Resources/keytao_ios_ime.json"),
                .process("Resources/keytao-logo.png"),
            ]
        ),
    ]
)
