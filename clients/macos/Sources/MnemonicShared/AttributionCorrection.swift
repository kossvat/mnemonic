import Foundation

/// An explicit project, including no project, for a transcript or exact range.
public struct AttributionCorrection: Encodable, Equatable, Sendable {
    public let stream: String?
    public let from: Date?
    public let to: Date?
    public let project: String?

    public init(stream: String? = nil, from: Date? = nil, to: Date? = nil, project: String?) {
        self.stream = stream
        self.from = from
        self.to = to
        self.project = project
    }

    public var isValid: Bool {
        if let stream {
            guard !stream.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  stream.utf8.count <= 4096,
                  stream.unicodeScalars.allSatisfy({ scalar in
                      !CharacterSet.controlCharacters.contains(scalar)
                  }) else { return false }
        }
        if let project, !ProjectKey.isValid(project) { return false }
        switch (from, to) {
        case (nil, nil): return stream != nil
        case (.some(let from), .some(let to)):
            let start = from.timeIntervalSince1970, end = to.timeIntervalSince1970
            return start.isFinite && end.isFinite && start.rounded(.towardZero) == start
                && end.rounded(.towardZero) == end && start < end
                && end - start < 3660 * 86400
                && start >= -62_135_596_800 && end < 253_402_300_800
        default: return false
        }
    }

    enum CodingKeys: String, CodingKey { case stream, from, to, project }

    public func encode(to encoder: Encoder) throws {
        guard isValid else {
            throw EncodingError.invalidValue(self, .init(
                codingPath: encoder.codingPath, debugDescription: "Invalid attribution correction"))
        }
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encodeIfPresent(stream, forKey: .stream)
        if let from, let to {
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime]
            try values.encode(formatter.string(from: from), forKey: .from)
            try values.encode(formatter.string(from: to), forKey: .to)
        }
        try values.encode(project, forKey: .project)
    }
}
