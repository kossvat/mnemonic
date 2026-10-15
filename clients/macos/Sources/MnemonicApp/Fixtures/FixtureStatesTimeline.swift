import Foundation
import MnemonicShared

extension FixtureStates {
    static func timelines() throws -> [FixtureState] {
        let overlap: [FixtureWire.Segment] = [.init(from: 540, to: 600), .init(agent: "codex", from: 550, to: 590)]
        let day = try FixtureWire.timeline(segments: overlap)
        let short: [FixtureWire.Segment] = [
            .init(from: 540, to: 541), .init(from: 550, to: 551), .init(from: 560, to: 561),
            .init(agent: "codex", from: 580, to: 580), .init(from: 0, to: 1)
        ]
        var states = [
            FixtureState(id: "timeline-focus", screen: .timeline, timeline: day,
                         expected: FixtureTimelineExpectations.byID["timeline-focus"]!),
            FixtureState(id: "timeline-day", screen: .timeline, timeline: day,
                         expected: FixtureTimelineExpectations.byID["timeline-day"]!),
            FixtureState(id: "timeline-one", screen: .timeline,
                         timeline: try FixtureWire.timeline(segments: [.init(from: 540, to: 600)]),
                         expected: FixtureTimelineExpectations.byID["timeline-one"]!),
            FixtureState(id: "timeline-short", screen: .timeline, timeline: try FixtureWire.timeline(segments: short),
                         height: 1_200,
                         expected: FixtureTimelineExpectations.byID["timeline-short"]!),
            FixtureState(id: "timeline-edge", screen: .timeline,
                         timeline: try FixtureWire.timeline(segments: [.init(from: 480, to: 600)]),
                         expected: FixtureTimelineExpectations.byID["timeline-edge"]!),
            FixtureState(id: "timeline-loading", screen: .timeline, loading: true, expected: FixtureTimelineExpectations.byID["timeline-loading"]!)
        ]
        states += failures(.timeline)
        guard let newYork = TimeZone(identifier: "America/New_York") else { throw AppFixturePreview.Failure.fixture }
        let many: [FixtureWire.Segment] = (0..<14).map { index in
            let agent = index.isMultiple(of: 2) ? "claude" : "codex"
            return FixtureWire.Segment(project: index, agent: agent, from: Double(480 + index * 10),
                                       to: Double(540 + index * 10))
        }
        // Twelve rows of one minute blocks, ten minutes apart, on one project, and two quiet projects.
        var crowded: [FixtureWire.Segment] = []
        for row in 0..<12 {
            for column in 0..<3 {
                let agent = (row + column).isMultiple(of: 2) ? "claude" : "codex"
                let start = Double(540 + column * 10)
                crowded.append(FixtureWire.Segment(agent: agent, from: start, to: start + 1))
            }
        }
        crowded.append(FixtureWire.Segment(project: 1, from: 660, to: 675))
        crowded.append(FixtureWire.Segment(project: 2, agent: "codex", from: 780, to: 795))
        states += [
            FixtureState(id: "timeline-activity-only", screen: .timeline,
                         timeline: try FixtureWire.timeline(projectCount: 2, owner: [
                             .init(from: 540, to: 560), .init(from: 560, to: 580, projects: [0, 1]),
                             .init(from: 600, to: 620, projects: [nil])]), height: 1_000,
                         expected: FixtureTimelineExpectations.byID["timeline-activity-only"]!),
            FixtureState(id: "timeline-empty", screen: .timeline, timeline: try FixtureWire.timeline(),
                         expected: FixtureTimelineExpectations.byID["timeline-empty"]!),
            FixtureState(id: "timeline-owner-unavailable", screen: .timeline,
                         timeline: try FixtureWire.timeline(segments: overlap, available: false),
                         expected: FixtureTimelineExpectations.byID["timeline-owner-unavailable"]!),
            FixtureState(id: "timeline-23h", screen: .timeline,
                         timeline: try FixtureWire.timeline(day: "2026-03-08", zone: newYork, segments: [.init(from: 60, to: 120)]),
                         timeZone: newYork, day: "2026-03-08",
                         expected: FixtureTimelineExpectations.byID["timeline-23h"]!),
            FixtureState(id: "timeline-25h", screen: .timeline,
                         timeline: try FixtureWire.timeline(day: "2026-11-01", zone: newYork, segments: [.init(from: 60, to: 120)]),
                         timeZone: newYork, day: "2026-11-01",
                         expected: FixtureTimelineExpectations.byID["timeline-25h"]!),
            FixtureState(id: "timeline-many", screen: .timeline,
                         timeline: try FixtureWire.timeline(projectCount: 14, long: true, segments: many), height: 3_500,
                         expected: FixtureTimelineExpectations.byID["timeline-many"]!),
            FixtureState(id: "timeline-crowded", screen: .timeline,
                         timeline: try FixtureWire.timeline(projectCount: 3, segments: crowded), height: 4_300,
                         expected: FixtureTimelineExpectations.byID["timeline-crowded"]!),
            FixtureState(id: "timeline-registry", screen: .timeline,
                         board: try FixtureWire.timelineRegistryBoard(), timeline: try FixtureWire.timelineRegistryDay(),
                         expected: FixtureTimelineExpectations.byID["timeline-registry"]!)
        ]
        return states
    }
}
