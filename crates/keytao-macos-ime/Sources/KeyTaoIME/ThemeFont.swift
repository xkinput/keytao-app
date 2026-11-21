import Cocoa

enum ThemeFontWeight: String, Codable {
    case ultraLight = "ultralight"
    case thin
    case light
    case regular
    case medium
    case semiBold = "semibold"
    case bold
    case heavy
    case black

    var nsWeight: NSFont.Weight {
        switch self {
        case .ultraLight:
            return .ultraLight
        case .thin:
            return .thin
        case .light:
            return .light
        case .regular:
            return .regular
        case .medium:
            return .medium
        case .semiBold:
            return .semibold
        case .bold:
            return .bold
        case .heavy:
            return .heavy
        case .black:
            return .black
        }
    }
}

extension NSFont {
    static func keytaoThemeFont(family: String?, size: CGFloat, weight: ThemeFontWeight) -> NSFont {
        keytaoThemeFontFromFamily(family, size: size, weight: weight.nsWeight)
            ?? .systemFont(ofSize: size, weight: weight.nsWeight)
    }

    static func keytaoThemeFont(family: String?, size: CGFloat) -> NSFont {
        keytaoThemeFontFromFamily(family, size: size, weight: .regular)
            ?? .systemFont(ofSize: size)
    }

    private static func keytaoThemeFontFromFamily(
        _ family: String?,
        size: CGFloat,
        weight: NSFont.Weight
    ) -> NSFont? {
        guard let family else {
            return nil
        }
        for candidate in family.split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) where !candidate.isEmpty {
            let descriptor = NSFontDescriptor(fontAttributes: [
                .family: candidate,
                .traits: [NSFontDescriptor.TraitKey.weight: weight.rawValue],
            ])
            if let font = NSFont(descriptor: descriptor, size: size) {
                return font
            }
            if let named = NSFont(name: candidate, size: size) {
                if let familyName = named.familyName {
                    let familyDescriptor = NSFontDescriptor(fontAttributes: [
                        .family: familyName,
                        .traits: [NSFontDescriptor.TraitKey.weight: weight.rawValue],
                    ])
                    if let weighted = NSFont(descriptor: familyDescriptor, size: size) {
                        return weighted
                    }
                }
                return named
            }
        }
        return nil
    }
}
