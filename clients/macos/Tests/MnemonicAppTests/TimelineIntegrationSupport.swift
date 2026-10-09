import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

@MainActor
final class TimelineIntegrationFixture {
    let root: URL
    let clock = ProjectTestClock()
    let projectSource: ProjectTestSource
    let reads: ProjectPlanRead<ProjectTimeline>
    let model: MnemonicAppModel
    let window = NSObject()
    let selectedDay: String

    init(day: String = "2026-06-17", today: String = "2026-06-17") throws {
        selectedDay = day
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-timeline-integration-\(UUID().uuidString)")
        let home = root.appendingPathComponent("profile")
        let user = root.appendingPathComponent("user")
        do {
            try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
            try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
            try "[ui]\nenabled = false\nport = 9\n".write(
                to: home.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
            let profile = try MnemonicProfile.resolve(
                environment: [MnemonicProfile.homeVariable: home.path],
                arguments: ["fixture-client"], userHome: user)
            clock.now = try XCTUnwrap(TimelineDay.date(today)).addingTimeInterval(43_200)
            let source = try ProjectTestSource(ProjectTestFixtures())
            projectSource = source
            let reads = ProjectPlanRead(try Self.snapshot(day: day))
            self.reads = reads
            let clock = clock
            let timeline = AppTimelineController(
                date: { clock.now }, read: { _ in await reads.read() },
                add: { _ in .failure(.off) }, remove: { _ in .failure(.off) })
            model = MnemonicAppModel(
                profile: profile, controller: projectTestController(source, clock: clock),
                timelineController: timeline, readStatus: { throw AppLegacyReadFailure.unavailable })
            model.activateWindow = {}
            model.route = .timeline(day)
        } catch {
            try? FileManager.default.removeItem(at: root)
            throw error
        }
    }

    static func snapshot(day: String, marker: String = "first") throws -> ProjectTimeline {
        let begin = try XCTUnwrap(TimelineDay.date(day))
        let end = try XCTUnwrap(Calendar.current.date(byAdding: .day, value: 1, to: begin))
        return ProjectTimeline(
            day: day, from: begin.timeIntervalSince1970, to: end.timeIntervalSince1970,
            projects: [TimelineProject(key: "demoapp", name: "Demo app")],
            segments: [TimelineSegment(
                id: marker, stream: "fixture-stream", project: "demoapp", agent: "claude", driver: "owner",
                from: begin.addingTimeInterval(3_600).timeIntervalSince1970,
                to: begin.addingTimeInterval(7_200).timeIntervalSince1970)],
            owner: [], ownerAvailable: true)
    }

    func close() {
        model.window(ObjectIdentifier(window), visible: false)
        Task {
            await reads.finishAll()
            await projectSource.finishLive()
        }
        try? FileManager.default.removeItem(at: root)
    }
}
