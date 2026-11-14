import Foundation

private func current(
    fingerprint: String = "keytao-core/1.2.5-alpha.5;bundled/1.2.5/42",
    markerSchemas: [String] = ["keytao"],
    coreVersion: String? = "1.2.5-alpha.5",
    bundleStamp: String? = "1.2.5/42",
    sharedStamp: String? = "1.2.5/42",
    schemas: [String] = ["keytao"],
    deploying: Bool = false
) -> Bool {
    let marker = try! JSONSerialization.data(withJSONObject: ["fingerprint": fingerprint, "schemas": markerSchemas])
    return KeyTaoIOSDeploymentState.isCurrent(
        marker: marker, coreVersion: coreVersion, bundleStamp: bundleStamp,
        sharedStamp: sharedStamp, schemas: schemas, deploying: deploying
    )
}

precondition(current(), "A matching deployment must be ready")
precondition(!current(fingerprint: "keytao-core/0.1.0;bundled/1.2.5/42"), "An old engine must require deployment")
precondition(!current(fingerprint: "keytao-core/1.2.5-alpha.5;bundled/1.2.4/41"), "An old app must require deployment")
precondition(!current(fingerprint: "keytao-core/1.2.5-alpha.5;bundled/1.2.5/41"), "A build-only update must require deployment")
precondition(!current(sharedStamp: "1.2.5/41"), "The app must refresh the shared data first")
precondition(!current(deploying: true), "The deployment sentinel must override a matching marker")
precondition(!current(markerSchemas: []), "The marker must cover every configured schema")
precondition(!current(schemas: ["keytao", "keytao_42"]), "Partial deployments must not be ready")
precondition(!current(schemas: []), "An empty installation must not be ready")
precondition(!current(coreVersion: nil), "A missing bundled engine version must not be ready")
precondition(!current(bundleStamp: nil), "A missing bundle stamp must not be ready")
precondition(!current(sharedStamp: nil), "An unstamped legacy copy must not be ready")
for marker in [nil, Data("invalid".utf8), Data("[\"keytao\"]".utf8)] as [Data?] {
    precondition(!KeyTaoIOSDeploymentState.isCurrent(
        marker: marker, coreVersion: "1.2.5-alpha.5", bundleStamp: "1.2.5/42",
        sharedStamp: "1.2.5/42", schemas: ["keytao"], deploying: false
    ), "Missing, malformed and legacy markers must not be ready")
}
print("Deployment state checks passed")
