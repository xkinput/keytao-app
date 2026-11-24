// swift-tools-version: 5.9

import PackageDescription

let package = Package(
  name: "webview_core",
  platforms: [
    .iOS("14.0")
  ],
  products: [
    .library(name: "webview-core", targets: ["webview_core"])
  ],
  dependencies: [
    .package(name: "FlutterFramework", path: "../FlutterFramework")
  ],
  targets: [
    .target(
      name: "webview_core",
      dependencies: [
        .product(name: "FlutterFramework", package: "FlutterFramework")
      ],
      resources: [
        .process("Resources")
      ]
    )
  ]
)
