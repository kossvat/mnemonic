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
        )
    ]
)
