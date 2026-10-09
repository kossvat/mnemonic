import Foundation

/// Time for commands, spent by running them.
///
/// A client runs the `mnemonic` command when the local API gives nothing.
/// A command that does not end (a daemon that holds its socket and never
/// answers) would hold whatever waits for it, so a run is cut when the
/// time is over, and the runs of one refresh share one budget: a command
/// that used it up leaves none for those after it.
public final class CommandBudget {
    private let deadline: Date
    private let now: () -> Date
    private let lock = NSLock()
    private var cutRuns = 0

    public init(limit: TimeInterval, now: @escaping () -> Date = Date.init) {
        self.now = now
        self.deadline = now().addingTimeInterval(limit)
    }

    /// What is left of the time.
    public var remaining: TimeInterval {
        max(0, deadline.timeIntervalSince(now()))
    }

    /// The runs that were cut at the limit or found no time left.
    public var cut: Int {
        lock.lock()
        defer { lock.unlock() }
        return cutRuns
    }

    /// What the command wrote to its standard output, or nil when it could
    /// not be started, was cut at the limit, or found no time left.
    public func output(
        of executable: URL, arguments: [String], environment: [String: String]? = nil
    ) -> Data? {
        let limit = remaining
        guard limit > 0 else {
            noteCut()
            return nil
        }
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        if let environment { process.environment = environment }
        let pipe = Pipe()
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        let exited = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in exited.signal() }
        do {
            try process.run()
        } catch {
            return nil
        }
        // The output is read while the command runs. A command that fills
        // the pipe waits for a reader, and a reader that first waits for
        // the command to end would wait for it forever.
        let reading = Reading()
        DispatchQueue.global(qos: .utility).async {
            reading.finish(pipe.fileHandleForReading.readDataToEndOfFile())
        }
        // Both the end of the output and the end of the command fall within
        // the limit: a command can close its output and go on running.
        let deadline = DispatchTime.now() + limit
        if let output = reading.wait(until: deadline), exited.wait(timeout: deadline) == .success {
            return output
        }
        noteCut()
        process.terminate()
        if exited.wait(timeout: .now() + Self.patience) != .success {
            kill(process.processIdentifier, SIGKILL)
            _ = exited.wait(timeout: .now() + Self.patience)
        }
        return nil
    }

    /// How long a command that was asked to end is waited for before it is
    /// killed, and once more after that.
    private static let patience: TimeInterval = 0.5

    private func noteCut() {
        lock.lock()
        cutRuns += 1
        lock.unlock()
    }

    /// The output of one run, handed from the thread that reads it to the
    /// one that waits for it.
    private final class Reading {
        private let done = DispatchSemaphore(value: 0)
        private let lock = NSLock()
        private var output: Data?

        func finish(_ data: Data) {
            lock.lock()
            output = data
            lock.unlock()
            done.signal()
        }

        func wait(until deadline: DispatchTime) -> Data? {
            guard done.wait(timeout: deadline) == .success else { return nil }
            lock.lock()
            defer { lock.unlock() }
            return output
        }
    }
}
