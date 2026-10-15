import CryptoKit
import Foundation
import MnemonicShared

/// The digest of everything a state is drawn from. A summary names what a
/// screen shows; the digest binds a state's name to its whole input, so an
/// input swapped for another, in any field or in any order, is caught even
/// where the summary does not name it.
enum FixtureStateDigest {
    private struct Inputs: Encodable {
        let screen: String
        let board: ProjectBoard?
        let live: LiveSnapshot?
        let detail: ProjectDetail?
        let timeline: ProjectTimeline?
        let failure: String?
        let loading: Bool
        let now: Double
        let zone: String
        let day: String
        let height: Double?
    }

    static func of(_ state: FixtureState) throws -> String {
        let inputs = Inputs(
            screen: state.screen.rawValue,
            board: state.board, live: state.live, detail: state.detail,
            timeline: state.timeline, failure: state.failure.map { failure in String(describing: failure) },
            loading: state.loading, now: state.now.timeIntervalSince1970, zone: state.timeZone.identifier,
            day: state.day, height: state.height.map { height in Double(height) })
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let digest = SHA256.hash(data: try encoder.encode(inputs))
        return digest.prefix(8).map { byte in String(format: "%02x", byte) }.joined()
    }
}
