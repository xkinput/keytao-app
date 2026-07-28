import UIKit

enum KeyTaoIOSKeyboardLayoutMode: Equatable {
    case full
    case oneHanded
    case split
}

struct KeyTaoIOSKeyboardLayoutPresentation: Equatable {
    var mode: KeyTaoIOSKeyboardLayoutMode
    var alternativeMode: KeyTaoIOSKeyboardLayoutMode
    var side: KeyTaoIOSKeyboardSide

    var isCompact: Bool {
        mode == .oneHanded
    }

    var isEnabled: Bool {
        mode != .full
    }

    var displayedMode: KeyTaoIOSKeyboardLayoutMode {
        mode == .full ? alternativeMode : mode
    }
}

final class KeyTaoIOSKeyboardLayoutStateStore {
    private let defaults: UserDefaults

    init(defaults: UserDefaults? = UserDefaults(suiteName: KeyTaoIOSPaths.appGroupIdentifier)) {
        self.defaults = defaults ?? .standard
    }

    func load(
        isLandscape: Bool,
        fallback: KeyTaoFloatingKeyboardProfile
    ) -> KeyTaoIOSKeyboardLayoutState {
        let prefix = orientationPrefix(isLandscape)
        let enabledKey = "\(prefix)_enabled"
        let scaleKey = "\(prefix)_scale"
        let side = defaults.string(forKey: "\(prefix)_side")
            .flatMap(KeyTaoIOSKeyboardSide.init(rawValue:))
            ?? .right
        return KeyTaoIOSKeyboardLayoutState(
            enabled: defaults.object(forKey: enabledKey) == nil
                ? fallback.enabled
                : defaults.bool(forKey: enabledKey),
            scale: defaults.object(forKey: scaleKey) == nil
                ? fallback.scale
                : defaults.double(forKey: scaleKey),
            side: side,
            orientation: KeyTaoFloatingOrientation(isLandscape: isLandscape)
        ).normalized()
    }

    func save(isLandscape: Bool, state: KeyTaoIOSKeyboardLayoutState) {
        let prefix = orientationPrefix(isLandscape)
        // The slot decides the floor, not whatever orientation the state was
        // built with: a landscape scale must never be stored under the portrait
        // key without being clamped to the portrait minimum first.
        var stored = state
        stored.orientation = KeyTaoFloatingOrientation(isLandscape: isLandscape)
        let normalized = stored.normalized()
        defaults.set(normalized.enabled, forKey: "\(prefix)_enabled")
        defaults.set(normalized.scale, forKey: "\(prefix)_scale")
        defaults.set(normalized.side.rawValue, forKey: "\(prefix)_side")
    }

    private func orientationPrefix(_ isLandscape: Bool) -> String {
        isLandscape ? "keytao_layout_v2_landscape" : "keytao_layout_v2_portrait"
    }
}

final class KeyTaoResizableKeyboardContainerView: UIView, UIGestureRecognizerDelegate {
    var onStateChanged: ((KeyTaoIOSKeyboardLayoutState, Bool) -> Void)?

    private var state = KeyTaoIOSKeyboardLayoutState(enabled: false, scale: 0.88)
    private var presentation = KeyTaoIOSKeyboardLayoutPresentation(
        mode: .full,
        alternativeMode: .oneHanded,
        side: .right
    )
    private var acceptsCurrentTouch = false
    private var dragStartWidth: CGFloat = 0
    private var dragStartScale: CGFloat = 1
    private let edgeTouchSize: CGFloat = 16
    private lazy var panGesture = UIPanGestureRecognizer(target: self, action: #selector(handlePan(_:)))

    override init(frame: CGRect) {
        super.init(frame: frame)
        setupGesture()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        setupGesture()
    }

    func configure(
        state: KeyTaoIOSKeyboardLayoutState,
        presentation: KeyTaoIOSKeyboardLayoutPresentation
    ) {
        self.state = state.normalized()
        self.presentation = presentation
        panGesture.isEnabled = presentation.isCompact
    }

    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldReceive touch: UITouch
    ) -> Bool {
        acceptsCurrentTouch = isOnResizableEdge(touch.location(in: self))
        return acceptsCurrentTouch
    }

    override func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        presentation.isCompact && acceptsCurrentTouch
    }

    @objc private func handlePan(_ gesture: UIPanGestureRecognizer) {
        switch gesture.state {
        case .began:
            dragStartWidth = bounds.width
            dragStartScale = state.scale
        case .changed:
            updateScale(translationX: gesture.translation(in: self).x, finished: false)
        case .ended:
            updateScale(translationX: gesture.translation(in: self).x, finished: true)
            acceptsCurrentTouch = false
        case .cancelled, .failed:
            onStateChanged?(state, true)
            acceptsCurrentTouch = false
        default:
            break
        }
    }

    private func setupGesture() {
        panGesture.delegate = self
        panGesture.cancelsTouchesInView = true
        panGesture.delaysTouchesBegan = false
        addGestureRecognizer(panGesture)
    }

    private func isOnResizableEdge(_ point: CGPoint) -> Bool {
        guard presentation.isCompact, bounds.width > 0 else {
            return false
        }
        switch presentation.side {
        case .left:
            return point.x >= bounds.width - edgeTouchSize
        case .right:
            return point.x <= edgeTouchSize
        }
    }

    private func updateScale(translationX: CGFloat, finished: Bool) {
        let baseWidth = dragStartWidth / max(dragStartScale, 0.01)
        let outwardTranslation = presentation.side == .left ? translationX : -translationX
        let nextScale = dragStartScale + outwardTranslation / max(baseWidth, 1)
        state = KeyTaoIOSKeyboardLayoutState(
            enabled: true,
            scale: nextScale,
            side: presentation.side,
            orientation: state.orientation
        ).normalized()
        onStateChanged?(state, finished)
    }
}
