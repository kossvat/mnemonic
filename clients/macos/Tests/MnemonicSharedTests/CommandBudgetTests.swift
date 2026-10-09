import XCTest
@testable import MnemonicShared

final class CommandBudgetTests: XCTestCase {
    private var scratch: URL!

    override func setUpWithError() throws {
        scratch = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-command-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: scratch)
    }

    private let shell = URL(fileURLWithPath: "/bin/sh")

    func testACommandThatEndsGivesItsOutput() {
        let budget = CommandBudget(limit: 10)
        let output = budget.output(of: shell, arguments: ["-c", "echo done"])
        XCTAssertEqual(output.map { bytes in String(decoding: bytes, as: UTF8.self) }, "done\n")
        XCTAssertEqual(budget.cut, 0)
    }

    func testOutputLargerThanAPipeIsGivenWhole() {
        let budget = CommandBudget(limit: 10)
        let output = budget.output(
            of: shell, arguments: ["-c", "head -c 300000 /dev/zero"])
        XCTAssertEqual(output?.count, 300_000)
    }

    func testACommandThatDoesNotEndIsCutAtTheLimitAndIsGone() throws {
        let pidFile = scratch.appendingPathComponent("pid")
        let budget = CommandBudget(limit: 0.4)
        let started = Date()
        let output = budget.output(
            of: shell, arguments: ["-c", "echo $$ > '\(pidFile.path)'; exec sleep 60"])
        let took = Date().timeIntervalSince(started)
        XCTAssertNil(output)
        XCTAssertEqual(budget.cut, 1)
        XCTAssertLessThan(took, 3, "the run outlived its limit")
        let text = try String(contentsOf: pidFile, encoding: .utf8)
        let pid = try XCTUnwrap(Int32(text.trimmingCharacters(in: .whitespacesAndNewlines)))
        var gone = false
        for _ in 0..<40 where !gone {
            gone = kill(pid, 0) != 0
            if !gone { usleep(50_000) }
        }
        XCTAssertTrue(gone, "the command that was cut still runs")
    }

    func testACommandThatIgnoresTheRequestToEndIsKilled() throws {
        let budget = CommandBudget(limit: 0.4)
        let started = Date()
        let output = budget.output(
            of: shell, arguments: ["-c", "trap '' TERM; while true; do sleep 1; done"])
        XCTAssertNil(output)
        XCTAssertLessThan(Date().timeIntervalSince(started), 4)
    }

    func testACommandThatClosesItsOutputAndGoesOnIsCutAtTheLimit() {
        let budget = CommandBudget(limit: 0.3)
        let started = Date()
        let output = budget.output(of: shell, arguments: ["-c", "exec 1>&-; exec sleep 5"])
        XCTAssertNil(output)
        XCTAssertEqual(budget.cut, 1)
        XCTAssertLessThan(Date().timeIntervalSince(started), 2.5, "the run outlived its limit")
    }

    func testTheRunsOfOneBudgetShareItsTime() {
        var clock = Date(timeIntervalSince1970: 1_000)
        let budget = CommandBudget(limit: 5, now: { clock })
        XCTAssertEqual(budget.remaining, 5)
        XCTAssertNotNil(budget.output(of: shell, arguments: ["-c", "true"]))
        clock = clock.addingTimeInterval(6)
        XCTAssertEqual(budget.remaining, 0)
        // No time is left: the command is not started at all.
        let marker = scratch.appendingPathComponent("started")
        XCTAssertNil(budget.output(of: shell, arguments: ["-c", "touch '\(marker.path)'"]))
        XCTAssertEqual(budget.cut, 1)
        XCTAssertFalse(FileManager.default.fileExists(atPath: marker.path))
    }

    func testACommandThatCannotBeStartedGivesNothing() {
        let budget = CommandBudget(limit: 5)
        XCTAssertNil(budget.output(
            of: scratch.appendingPathComponent("no-such-command"), arguments: []))
        XCTAssertEqual(budget.cut, 0)
    }
}
