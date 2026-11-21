import Cocoa
import Foundation

private func fail(_ message: String, file: StaticString = #file, line: UInt = #line) -> Never {
    fputs("FAIL (\(file):\(line)): \(message)\n", stderr)
    exit(1)
}

private func expect(_ condition: @autoclosure () -> Bool, _ message: String, file: StaticString = #file, line: UInt = #line) {
    if !condition() {
        fail(message, file: file, line: line)
    }
}

for family in ["PingFang SC", "Songti SC", "Arial", "Menlo"] {
    let regular = NSFont.keytaoThemeFont(family: family, size: 20, weight: .regular)
    let semibold = NSFont.keytaoThemeFont(family: family, size: 20, weight: .semiBold)
    let bold = NSFont.keytaoThemeFont(family: family, size: 20, weight: .bold)

    expect(
        semibold.fontName != regular.fontName,
        "\(family) semibold resolved to regular face \(semibold.fontName)"
    )
    expect(
        bold.fontName != regular.fontName,
        "\(family) bold resolved to regular face \(bold.fontName)"
    )
}

let postScriptBold = NSFont.keytaoThemeFont(family: "Menlo-Regular", size: 20, weight: .bold)
let postScriptRegular = NSFont.keytaoThemeFont(family: "Menlo-Regular", size: 20, weight: .regular)
expect(
    postScriptBold.fontName != postScriptRegular.fontName,
    "PostScript regular name should still resolve bold face within its family, got \(postScriptBold.fontName)"
)

let fallbackBold = NSFont.keytaoThemeFont(family: "NonExistentKeyTaoFont123, Arial", size: 20, weight: .bold)
expect(
    fallbackBold.fontName == "Arial-BoldMT",
    "Comma fallback should pick Arial-BoldMT, got \(fallbackBold.fontName)"
)

let systemBold = NSFont.keytaoThemeFont(family: "NonExistentKeyTaoFont123", size: 20, weight: .bold)
let systemRegular = NSFont.keytaoThemeFont(family: nil, size: 20, weight: .regular)
expect(
    systemBold.fontName != systemRegular.fontName,
    "Missing family should fall back to weighted system font, got \(systemBold.fontName)"
)

print("ThemeFontTests passed")
