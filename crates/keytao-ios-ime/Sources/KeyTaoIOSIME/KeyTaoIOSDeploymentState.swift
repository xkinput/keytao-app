import Foundation

enum KeyTaoIOSDeploymentState {
    private struct Marker: Decodable {
        let fingerprint: String
        let schemas: [String]
    }

    static func isCurrent(
        marker: Data?,
        coreVersion: String?,
        bundleStamp: String?,
        sharedStamp: String?,
        schemas: [String],
        deploying: Bool
    ) -> Bool {
        guard !deploying, !schemas.isEmpty,
              let coreVersion, !coreVersion.isEmpty,
              let bundleStamp, !bundleStamp.isEmpty,
              sharedStamp == bundleStamp,
              let marker, let deployed = try? JSONDecoder().decode(Marker.self, from: marker) else {
            return false
        }
        return deployed.fingerprint == "keytao-core/\(coreVersion);bundled/\(bundleStamp)"
            && schemas.allSatisfy { deployed.schemas.contains($0) }
    }
}
