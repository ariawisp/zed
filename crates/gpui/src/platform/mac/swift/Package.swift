// swift-tools-version:6.2
import PackageDescription

let package = Package(
    name: "GPUIAppKit",
    platforms: [
        .macOS(.v26)
    ],
    products: [
        .library(name: "GPUIAppKit", type: .dynamic, targets: ["GPUIAppKit"])
    ],
    targets: [
        // Clang/C target to expose the C ABI header to Swift
        .target(
            name: "GPUIFFI",
            path: "GPUIFFI",
            publicHeadersPath: "include"
        ),
        .target(
            name: "GPUIAppKit",
            dependencies: ["GPUIFFI"],
            path: "Sources/GPUIAppKit"
        )
    ]
)
