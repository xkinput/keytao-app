import Cocoa

final class ModeIndicatorPanel: NSPanel {
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

    func show(asciiMode: Bool, near cursorRect: NSRect) {
        let theme = ImeThemeManager.shared.theme()
        apply(theme)

        label.stringValue = asciiMode ? theme.modeHint.englishText : theme.modeHint.chineseText
        let size = NSSize(width: theme.modeHint.width, height: theme.modeHint.height)
        let screen = NSScreen.screen(containing: cursorRect) ?? NSScreen.main
        let visibleFrame = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let anchor = cursorRect.isUsableTextInputRect
            ? cursorRect
            : NSRect(origin: NSEvent.mouseLocation, size: .zero)

        var origin = NSPoint(
            x: anchor.midX - size.width / 2,
            y: anchor.minY - size.height - 8
        )
        origin.x = min(max(origin.x, visibleFrame.minX + 8), visibleFrame.maxX - size.width - 8)
        origin.y = min(max(origin.y, visibleFrame.minY + 8), visibleFrame.maxY - size.height - 8)

        setFrame(NSRect(origin: origin, size: size), display: true, animate: false)
        orderFront(nil)

        hideTimer?.invalidate()
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
            label.leadingAnchor.constraint(equalTo: containerView.leadingAnchor),
            label.trailingAnchor.constraint(equalTo: containerView.trailingAnchor),
            label.topAnchor.constraint(equalTo: containerView.topAnchor),
            label.bottomAnchor.constraint(equalTo: containerView.bottomAnchor),
        ])

        contentView = containerView
    }

    private func apply(_ theme: ImeTheme) {
        hasShadow = theme.modeHint.shadow
        containerView.layer?.backgroundColor = theme.modeHint.background.cgColor
        containerView.layer?.borderColor = theme.modeHint.borderColor.cgColor
        containerView.layer?.borderWidth = theme.modeHint.borderWidth
        containerView.layer?.cornerRadius = theme.modeHint.cornerRadius
        label.font = .systemFont(ofSize: theme.modeHint.fontSize, weight: .semibold)
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
