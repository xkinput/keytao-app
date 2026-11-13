import Foundation
import Darwin

final class KeyTaoExecutableUpdateGuard {
    static let shared = KeyTaoExecutableUpdateGuard()

    private let executableURL = Bundle.main.executableURL
    private let launchIdentity: Identity?

    private init() {
        launchIdentity = executableURL.flatMap(Identity.init)
    }

    var hasChanged: Bool {
        guard let launchIdentity, let executableURL,
              let current = Identity(url: executableURL) else { return false }
        return current != launchIdentity
    }

    private struct Identity: Equatable {
        let device: dev_t
        let inode: ino_t
        let seconds: Int
        let nanoseconds: Int

        init?(url: URL) {
            var info = stat()
            guard stat(url.path, &info) == 0 else { return nil }
            device = info.st_dev
            inode = info.st_ino
            seconds = info.st_mtimespec.tv_sec
            nanoseconds = info.st_mtimespec.tv_nsec
        }
    }
}
