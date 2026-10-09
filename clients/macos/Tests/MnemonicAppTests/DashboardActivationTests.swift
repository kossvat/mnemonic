import XCTest
@testable import MnemonicApp

@MainActor
final class DashboardActivationTests: XCTestCase {
    func testSettingsOnlyOpensDashboardAndCoalescesPendingLinks() {
        let world = ActivationWorld()
        let settings = world.addWindow(identifier: "settings")
        world.registerOpener()

        world.activation.activate()
        world.activation.activate()

        XCTAssertEqual(world.openRequests, 1)
        XCTAssertEqual(world.applicationActivations, 2)
        XCTAssertEqual(settings.fronts, 0)
        XCTAssertEqual(settings.deminiaturizations, 0)

        let dashboard = world.attachDashboard()
        world.activation.activate()

        XCTAssertEqual(world.openRequests, 1)
        XCTAssertEqual(dashboard.fronts, 1)
        XCTAssertEqual(settings.fronts, 0)
    }

    func testExistingDashboardIsSelectedInsteadOfEarlierSettingsWindow() {
        let world = ActivationWorld()
        let settings = world.addWindow(identifier: "settings")
        let dashboard = world.attachDashboard()
        world.registerOpener()

        world.activation.activate()

        XCTAssertEqual(world.openRequests, 0)
        XCTAssertEqual(settings.fronts, 0)
        XCTAssertEqual(dashboard.fronts, 1)
        XCTAssertEqual(dashboard.deminiaturizations, 0)
        XCTAssertEqual(world.applicationActivations, 1)
    }

    func testMiniaturizedDashboardRestoresBeforeItIsBroughtForward() {
        let world = ActivationWorld()
        let dashboard = world.attachDashboard(miniaturized: true)
        world.registerOpener()

        world.activation.activate()

        XCTAssertEqual(world.openRequests, 0)
        XCTAssertEqual(dashboard.deminiaturizations, 1)
        XCTAssertEqual(dashboard.actions, ["deminiaturize", "front"])
        XCTAssertEqual(world.applicationActivations, 1)
    }

    func testClosedDashboardCanReopenEvenWithoutAnotherLinkWhileItWasOpen() {
        let world = ActivationWorld()
        let settings = world.addWindow(identifier: "settings")
        world.registerOpener()
        world.activation.activate()
        let closedDashboard = world.attachDashboard()

        // The visibility probe clears this identity on the native close event.
        // Keep the closed window in the list to also cover retained windows.
        closedDashboard.identifier = nil
        world.activation.activate()
        world.activation.activate()

        XCTAssertEqual(world.openRequests, 2)
        XCTAssertEqual(closedDashboard.fronts, 0)
        XCTAssertEqual(settings.fronts, 0)
    }

    func testStartupActivationBeforeFirstWindowDoesNotRequestADuplicate() {
        let world = ActivationWorld()

        world.activation.activate()
        XCTAssertEqual(world.openRequests, 0)

        // SwiftUI creates the initial scene after --open-url selects the route.
        let dashboard = world.attachDashboard()
        world.registerOpener()
        world.activation.activate()

        XCTAssertEqual(world.openRequests, 0)
        XCTAssertEqual(dashboard.fronts, 1)
        XCTAssertEqual(world.applicationActivations, 2)
    }

    func testEarlyActivationDoesNotConsumeARegisteredOpenerLater() {
        let world = ActivationWorld()
        world.activation.activate()
        world.registerOpener()

        world.activation.activate()

        XCTAssertEqual(world.openRequests, 1)
        XCTAssertEqual(world.applicationActivations, 2)
    }
}

@MainActor
private final class ActivationWindow {
    var identifier: String?
    var miniaturized: Bool
    var fronts = 0
    var deminiaturizations = 0
    var actions: [String] = []

    init(identifier: String?, miniaturized: Bool) {
        self.identifier = identifier
        self.miniaturized = miniaturized
    }

    var actionsForActivation: DashboardActivation.Window {
        DashboardActivation.Window(identifier: identifier,
                                   isMiniaturized: miniaturized,
                                   deminiaturize: { [unowned self] in
                                       deminiaturizations += 1
                                       miniaturized = false
                                       actions.append("deminiaturize")
                                   }, orderFront: { [unowned self] in
                                       fronts += 1
                                       actions.append("front")
                                   })
    }
}

@MainActor
private final class ActivationWorld {
    var windows: [ActivationWindow] = []
    var openRequests = 0
    var applicationActivations = 0
    lazy var activation = DashboardActivation(windows: { [unowned self] in
        windows.map { window in window.actionsForActivation }
    }, activateApplication: { [unowned self] in applicationActivations += 1 })

    func registerOpener() {
        activation.registerOpener { [unowned self] in openRequests += 1 }
    }

    @discardableResult
    func addWindow(identifier: String?, miniaturized: Bool = false) -> ActivationWindow {
        let window = ActivationWindow(identifier: identifier, miniaturized: miniaturized)
        windows.append(window)
        return window
    }

    func attachDashboard(miniaturized: Bool = false) -> ActivationWindow {
        let window = addWindow(identifier: DashboardActivation.windowIdentifier.rawValue,
                               miniaturized: miniaturized)
        activation.windowDidAttach()
        return window
    }
}
