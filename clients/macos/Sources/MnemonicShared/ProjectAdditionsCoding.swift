import Foundation

// The fields added to the project routes by design 9.1 and 9.2 keep an
// omitted key apart from a recorded null, so a block is not drawn for a
// daemon that does not serve its value.

extension ProjectBoard {
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        projects = try container.decode([ProjectBoardRow].self, forKey: .projects)
        week = try container.decodePresence(ProjectWeek.self, forKey: .week)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(projects, forKey: .projects)
        try container.encodePresence(week, forKey: .week)
    }
}

extension ProjectBoardRow {
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        key = try container.decode(String.self, forKey: .key)
        name = try container.decode(String.self, forKey: .name)
        state = try container.decode(ProjectState.self, forKey: .state)
        mergedInto = try container.decodeIfPresent(String.self, forKey: .mergedInto)
        lastActiveAt = try container.decodeIfPresent(Int.self, forKey: .lastActiveAt)
        week = try container.decode(ProjectWeek.self, forKey: .week)
        plan = try container.decodeIfPresent(LivePlan.self, forKey: .plan)
        planMessage = try container.decodeIfPresent(String.self, forKey: .planMessage)
        gateAt = try container.decodePresence(Int.self, forKey: .gateAt)
        planProvider = try container.decodePresence(ProjectPlanProvider.self, forKey: .planProvider)
        planCounts = try container.decodePresence([String: Int].self, forKey: .planCounts)
        planWeeklyCount = try container.decodePresence(Int.self, forKey: .planWeeklyCount)
        openUnits = try container.decodePresence(Int.self, forKey: .openUnits)
        waitingOnOwner = try container.decodePresence([String].self, forKey: .waitingOnOwner)
        lastClosed = try container.decodePresence(ProjectLastClosed.self, forKey: .lastClosed)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(key, forKey: .key)
        try container.encode(name, forKey: .name)
        try container.encode(state, forKey: .state)
        try container.encodeIfPresent(mergedInto, forKey: .mergedInto)
        try container.encodeIfPresent(lastActiveAt, forKey: .lastActiveAt)
        try container.encode(week, forKey: .week)
        try container.encodeIfPresent(plan, forKey: .plan)
        try container.encodeIfPresent(planMessage, forKey: .planMessage)
        try container.encodePresence(gateAt, forKey: .gateAt)
        try container.encodePresence(planProvider, forKey: .planProvider)
        try container.encodePresence(planCounts, forKey: .planCounts)
        try container.encodePresence(planWeeklyCount, forKey: .planWeeklyCount)
        try container.encodePresence(openUnits, forKey: .openUnits)
        try container.encodePresence(waitingOnOwner, forKey: .waitingOnOwner)
        try container.encodePresence(lastClosed, forKey: .lastClosed)
    }
}

extension ProjectLastClosed {
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        title = try container.decode(String.self, forKey: .title)
        closedAt = try container.decodePresence(Int.self, forKey: .closedAt)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(title, forKey: .title)
        try container.encodePresence(closedAt, forKey: .closedAt)
    }
}

extension ProjectPlanUnit {
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        title = try container.decode(String.self, forKey: .title)
        done = try container.decode(Int.self, forKey: .done)
        total = try container.decode(Int.self, forKey: .total)
        stage = try container.decode(ProjectPlanStage.self, forKey: .stage)
        stale = try container.decode(Bool.self, forKey: .stale)
        currentTask = try container.decodeIfPresent(ProjectCurrentTask.self, forKey: .currentTask)
        counts = try container.decode([String: Int].self, forKey: .counts)
        waitingOnOwner = try container.decode([String].self, forKey: .waitingOnOwner)
        weeklyCount = try container.decode(Int.self, forKey: .weeklyCount)
        gate = try container.decode(ProjectGate.self, forKey: .gate)
        provider = try container.decode(ProjectPlanProvider.self, forKey: .provider)
        checkouts = try container.decode(Int.self, forKey: .checkouts)
        asOf = try container.decode(Int.self, forKey: .asOf)
        tasks = try container.decode([ProjectTask].self, forKey: .tasks)
        error = try container.decodeIfPresent(String.self, forKey: .error)
        closed = try container.decodePresence(Bool.self, forKey: .closed)
        closedAt = try container.decodePresence(Int.self, forKey: .closedAt)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(title, forKey: .title)
        try container.encode(done, forKey: .done)
        try container.encode(total, forKey: .total)
        try container.encode(stage, forKey: .stage)
        try container.encode(stale, forKey: .stale)
        try container.encodeIfPresent(currentTask, forKey: .currentTask)
        try container.encode(counts, forKey: .counts)
        try container.encode(waitingOnOwner, forKey: .waitingOnOwner)
        try container.encode(weeklyCount, forKey: .weeklyCount)
        try container.encode(gate, forKey: .gate)
        try container.encode(provider, forKey: .provider)
        try container.encode(checkouts, forKey: .checkouts)
        try container.encode(asOf, forKey: .asOf)
        try container.encode(tasks, forKey: .tasks)
        try container.encodeIfPresent(error, forKey: .error)
        try container.encodePresence(closed, forKey: .closed)
        try container.encodePresence(closedAt, forKey: .closedAt)
    }
}

extension ProjectGate {
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        status = try container.decode(ProjectGateStatus.self, forKey: .status)
        at = try container.decodeIfPresent(Int.self, forKey: .at)
        failedRuns = try container.decode(Int.self, forKey: .failedRuns)
        result = try container.decodePresence(ProjectGateResult.self, forKey: .result)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(status, forKey: .status)
        try container.encodeIfPresent(at, forKey: .at)
        try container.encode(failedRuns, forKey: .failedRuns)
        try container.encodePresence(result, forKey: .result)
    }
}

extension ProjectHistoryEntry {
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        title = try container.decode(String.self, forKey: .title)
        at = try container.decode(Int.self, forKey: .at)
        kind = try container.decodePresence(String.self, forKey: .kind)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(title, forKey: .title)
        try container.encode(at, forKey: .at)
        try container.encodePresence(kind, forKey: .kind)
    }
}

extension LiveSession {
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        agent = try container.decode(String.self, forKey: .agent)
        driver = try container.decode(String.self, forKey: .driver)
        state = try container.decode(LiveState.self, forKey: .state)
        startedAt = try container.decode(Int.self, forKey: .startedAt)
        lastRecordAt = try container.decodePresence(Int.self, forKey: .lastRecordAt)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(agent, forKey: .agent)
        try container.encode(driver, forKey: .driver)
        try container.encode(state, forKey: .state)
        try container.encode(startedAt, forKey: .startedAt)
        try container.encodePresence(lastRecordAt, forKey: .lastRecordAt)
    }
}
