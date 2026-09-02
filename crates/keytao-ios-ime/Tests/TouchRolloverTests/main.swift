import Foundation

// Container-only coverage: this test does not exercise KeyTaoIOSKeyboardView touch callbacks.
var failures: [String] = []

func expect(_ condition: Bool, _ message: String, line: Int = #line) {
    if !condition {
        failures.append("line \(line): \(message)")
    }
}

let firstTouch = NSObject()
let secondTouch = NSObject()
let firstIdentifier = ObjectIdentifier(firstTouch)
let secondIdentifier = ObjectIdentifier(secondTouch)
struct TestTouchState {
    var key: String
    var currentLocation: Int
}

var activeTouches = KeyTaoTouchRolloverStateMachine<TestTouchState>()
var output: [String] = []

activeTouches.begin(TestTouchState(key: "A", currentLocation: 0), for: firstIdentifier)
activeTouches.begin(TestTouchState(key: "B", currentLocation: 0), for: secondIdentifier)
activeTouches.move(secondIdentifier) { $0.currentLocation = 1 }
output += activeTouches.finish(firstIdentifier) { $0.key }
output += activeTouches.finish(secondIdentifier) { $0.key }

expect(output == ["A", "B"], "A down, B down, A up, B up must commit A then B")
expect(activeTouches.isEmpty, "all ended touches must be removed independently")

if failures.isEmpty {
    print("touch rollover tests passed")
} else {
    for failure in failures {
        FileHandle.standardError.write(Data("FAIL \(failure)\n".utf8))
    }
    exit(1)
}
