// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MnemonicBar",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "MnemonicBar",
            path: "Sources/MnemonicBar"
        )
    ]
)
