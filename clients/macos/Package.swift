// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Mnemonic",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "MnemonicBar", targets: ["MnemonicBar"]),
        .executable(name: "MnemonicApp", targets: ["MnemonicApp"]),
    ],
    targets: [
        // Shared design tokens — Theme palette, typography, spacing, motion.
        // Lives here so both the menu-bar widget and the standalone app speak
        // the same visual language without duplicating constants.
        .target(
            name: "MnemonicShared",
            path: "Sources/MnemonicShared"
        ),
        .executableTarget(
            name: "MnemonicBar",
            dependencies: ["MnemonicShared"],
            path: "Sources/MnemonicBar"
        ),
        .executableTarget(
            name: "MnemonicApp",
            dependencies: ["MnemonicShared"],
            path: "Sources/MnemonicApp"
        ),
        // The model of the card of memory today, on the fixture file the
        // daemon's tests print (Tests/Fixtures/recall-day.json).
        .testTarget(
            name: "MnemonicSharedTests",
            dependencies: ["MnemonicShared"],
            path: "Tests/MnemonicSharedTests"
        ),
        // The widget at rest: what it assigns, asks and mounts while its
        // popover is closed, over a source and a clock the tests stand in for.
        .testTarget(
            name: "MnemonicBarTests",
            dependencies: ["MnemonicBar", "MnemonicShared"],
            path: "Tests/MnemonicBarTests"
        ),
        .testTarget(
            name: "MnemonicAppTests",
            dependencies: ["MnemonicApp", "MnemonicShared"],
            path: "Tests/MnemonicAppTests"
        )
    ]
)
