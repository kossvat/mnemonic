/// A field keeps the distinction between an omitted key and a recorded null.
public enum FieldPresence<Value> {
    case missing
    case null
    case value(Value)

    public var value: Value? {
        guard case .value(let value) = self else { return nil }
        return value
    }
}

extension FieldPresence: Equatable where Value: Equatable {}
extension FieldPresence: Sendable where Value: Sendable {}

extension KeyedDecodingContainer {
    public func decodePresence<Value: Decodable>(_ type: Value.Type, forKey key: Key) throws -> FieldPresence<Value> {
        guard contains(key) else { return .missing }
        if try decodeNil(forKey: key) { return .null }
        return .value(try decode(type, forKey: key))
    }
}

extension KeyedEncodingContainer {
    /// Writes back what was read: no key, a null, or the value.
    public mutating func encodePresence<Value: Encodable>(_ presence: FieldPresence<Value>, forKey key: Key) throws {
        switch presence {
        case .missing: break
        case .null: try encodeNil(forKey: key)
        case .value(let value): try encode(value, forKey: key)
        }
    }
}
