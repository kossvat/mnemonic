extension ProjectDetail {
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        key = try container.decode(String.self, forKey: .key)
        name = try container.decode(String.self, forKey: .name)
        state = try container.decode(ProjectState.self, forKey: .state)
        plan = try container.decode(ProjectPlan.self, forKey: .plan)
        sessions = try container.decode([LiveSession].self, forKey: .sessions)
        gateDetail = try container.decodeIfPresent([String: ProjectGateDetail].self, forKey: .gateDetail)
        time = try container.decode([ProjectDay].self, forKey: .time)
        ownerEstimate = try container.decode(Bool.self, forKey: .ownerEstimate)
        history = try container.decode([ProjectHistoryEntry].self, forKey: .history)
        topTermsPresence = try container.decodePresence([ProjectTerm].self, forKey: .topTerms)
        topTerms = topTermsPresence.value
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(key, forKey: .key)
        try container.encode(name, forKey: .name)
        try container.encode(state, forKey: .state)
        try container.encode(plan, forKey: .plan)
        try container.encode(sessions, forKey: .sessions)
        try container.encodeIfPresent(gateDetail, forKey: .gateDetail)
        try container.encode(time, forKey: .time)
        try container.encode(ownerEstimate, forKey: .ownerEstimate)
        try container.encode(history, forKey: .history)
        try container.encodePresence(topTermsPresence, forKey: .topTerms)
    }
}
