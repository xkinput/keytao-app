import Cocoa

final class ModeIndicatorPanel: NSPanel {
    private let rootView = NSView()
    private let containerView = NSView()
    private let label = NSTextField(labelWithString: "")
    private var hideTimer: Timer?

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 72, height: 48),
            styleMask: [.nonactivatingPanel, .borderless],
            backing: .buffered,
            defer: false
        )
        configure()
    }

    func show(
        modeHint: KeyTaoModeHintModel,
        windowLevel: NSWindow.Level,
        near cursorRect: NSRect
    ) {
        let theme = ImeThemeManager.shared.theme()
        level = windowLevel
        apply(theme)

        label.stringValue = modeHint.text
        let margin = theme.modeHint.shadow ? (theme.modeHint.shadowStyle?.margin ?? 0) : 0
        let padding = theme.modeHint.fontSize / 3
        let contentSize = label.intrinsicContentSize
        let bodySize = NSSize(width: max(theme.modeHint.width, contentSize.width + padding * 2), height: max(theme.modeHint.height, contentSize.height + padding * 2))
        let size = NSSize(width: bodySize.width + margin * 2, height: bodySize.height + margin * 2)
        let screen = NSScreen.screen(containing: cursorRect) ?? NSScreen.main
        let visibleFrame = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let anchor = cursorRect.isUsableTextInputRect
            ? cursorRect
            : NSRect(origin: NSEvent.mouseLocation, size: .zero)

        var origin = NSPoint(
            x: anchor.midX - size.width / 2,
            y: anchor.minY - bodySize.height - margin - 8
        )
        origin.x = min(max(origin.x, visibleFrame.minX + 8), visibleFrame.maxX - size.width - 8)
        origin.y = min(max(origin.y, visibleFrame.minY + 8), visibleFrame.maxY - size.height - 8)

        setFrame(NSRect(origin: origin, size: size), display: true, animate: false)
        containerView.frame = NSRect(origin: NSPoint(x: margin, y: margin), size: bodySize)
        rootView.layer?.shadowPath = CGPath(roundedRect: containerView.frame, cornerWidth: theme.modeHint.cornerRadius, cornerHeight: theme.modeHint.cornerRadius, transform: nil)
        orderFront(nil)

        hideTimer?.invalidate()
        hideTimer = nil
        hideTimer = Timer.scheduledTimer(withTimeInterval: theme.modeHint.duration, repeats: false) { [weak self] _ in
            self?.orderOut(nil)
        }
    }

    override func orderOut(_ sender: Any?) {
        hideTimer?.invalidate()
        hideTimer = nil
        super.orderOut(sender)
    }

    private func configure() {
        isFloatingPanel = true
        level = .popUpMenu
        isOpaque = false
        backgroundColor = .clear
        isMovable = false
        hidesOnDeactivate = false

        containerView.wantsLayer = true
        containerView.layer?.masksToBounds = true

        label.alignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        containerView.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: containerView.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: containerView.centerYAnchor),
        ])

        rootView.wantsLayer = true
        rootView.addSubview(containerView)
        contentView = rootView
    }

    private func apply(_ theme: ImeTheme) {
        hasShadow = false
        let shadow = theme.modeHint.shadowStyle
        rootView.layer?.shadowOpacity = theme.modeHint.shadow ? 1 : 0
        rootView.layer?.shadowColor = shadow?.color.cgColor
        rootView.layer?.shadowOffset = CGSize(width: 0, height: -(shadow?.offsetY ?? 0))
        rootView.layer?.shadowRadius = (shadow?.blurRadius ?? 0) / 2
        containerView.layer?.backgroundColor = theme.modeHint.background.cgColor
        containerView.layer?.borderColor = theme.modeHint.borderColor.cgColor
        containerView.layer?.borderWidth = theme.modeHint.borderWidth
        containerView.layer?.cornerRadius = theme.modeHint.cornerRadius
        label.font = .keytaoThemeFont(family: theme.font.effectiveFamily, size: theme.modeHint.fontSize, weight: theme.font.weight)
        label.textColor = theme.modeHint.foreground.nsColor
    }
}

extension NSScreen {
    static func screen(containing rect: NSRect) -> NSScreen? {
        guard rect.isUsableTextInputRect else {
            return nil
        }
        let lookup = rect.textInputLookupRect
        return screens.first {
            $0.frame.intersects(lookup) || $0.frame.contains(NSPoint(x: rect.minX, y: rect.minY))
        }
    }
}
