import Flutter
import UIKit
import XCTest
@testable import Runner

class RunnerTests: XCTestCase {

  func testRimeDataRefreshesForUpgradeAndIncompleteCopy() {
    XCTAssertTrue(KeyTaoIOSBundledData.shouldRefreshRimeData(
      storedStamp: nil, bundleStamp: "1.2.5/42", hasDefaultYaml: true
    ))
    XCTAssertTrue(KeyTaoIOSBundledData.shouldRefreshRimeData(
      storedStamp: "1.2.5/41", bundleStamp: "1.2.5/42", hasDefaultYaml: true
    ))
    XCTAssertTrue(KeyTaoIOSBundledData.shouldRefreshRimeData(
      storedStamp: "1.2.4/42", bundleStamp: "1.2.5/42", hasDefaultYaml: true
    ))
    XCTAssertTrue(KeyTaoIOSBundledData.shouldRefreshRimeData(
      storedStamp: "1.2.5/42", bundleStamp: "1.2.5/42", hasDefaultYaml: false
    ))
    XCTAssertTrue(KeyTaoIOSBundledData.shouldRefreshRimeData(
      storedStamp: "1.2.5/42\n", bundleStamp: "1.2.5/42", hasDefaultYaml: true
    ))
    XCTAssertFalse(KeyTaoIOSBundledData.shouldRefreshRimeData(
      storedStamp: "1.2.5/42", bundleStamp: "1.2.5/42", hasDefaultYaml: true
    ))
  }

  func testKeyboardReseedsMissingAndUneditedDefaults() {
    XCTAssertTrue(KeyTaoIOSBundledData.shouldRefreshKeyboard(
      existingHash: nil, bundledHash: "current", seededHash: nil
    ))
    XCTAssertTrue(KeyTaoIOSBundledData.shouldRefreshKeyboard(
      existingHash: "previous", bundledHash: "current", seededHash: "previous\n"
    ))
    for hash in KeyTaoIOSBundledData.legacyDefaultKeyboardHashes {
      XCTAssertTrue(KeyTaoIOSBundledData.shouldRefreshKeyboard(
        existingHash: hash, bundledHash: "current", seededHash: nil
      ), hash)
    }
  }

  func testKeyboardPreservesUserEditsAndCurrentDefault() {
    XCTAssertFalse(KeyTaoIOSBundledData.shouldRefreshKeyboard(
      existingHash: "edited", bundledHash: "current", seededHash: "previous"
    ))
    XCTAssertFalse(KeyTaoIOSBundledData.shouldRefreshKeyboard(
      existingHash: "edited", bundledHash: "current", seededHash: nil
    ))
    XCTAssertFalse(KeyTaoIOSBundledData.shouldRefreshKeyboard(
      existingHash: "current", bundledHash: "current", seededHash: "previous"
    ))
  }

}
