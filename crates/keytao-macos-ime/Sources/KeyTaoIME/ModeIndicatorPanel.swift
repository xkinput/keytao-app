import Cocoa

final class ModeIndicatorPanel: NSPanel {
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
        label.stringValue = asciiMode ? "英" : "中"
        let size = frame.size
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
        hideTimer = Timer.scheduledTimer(withTimeInterval: 0.75, repeats: false) { [weak self] _ in
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
        hasShadow = true
        isMovable = false
        hidesOnDeactivate = false

        let container = NSVisualEffectView()
        container.material = .hudWindow
        container.blendingMode = .behindWindow
        container.state = .active
        container.wantsLayer = true
        container.layer?.cornerRadius = 12
        container.layer?.masksToBounds = true

        label.alignment = .center
        label.font = .systemFont(ofSize: 24, weight: .semibold)
        label.textColor = .labelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            label.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            label.topAnchor.constraint(equalTo: container.topAnchor),
            label.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])

        contentView = container
    }
}

extension NSScreen {
    static var globalMaxY: CGFloat {
        screens.map(\.frame.maxY).max() ?? (main?.frame.maxY ?? 0)
    }

    static func screen(containing rect: NSRect) -> NSScreen? {
        guard rect.isUsableTextInputRect else {
            return nil
        }
        return screens.first { $0.frame.intersects(rect) }
    }
}
