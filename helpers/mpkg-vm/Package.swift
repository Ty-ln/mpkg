// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "mpkg-vm",
    platforms: [.macOS(.v11)], // Virtualization.framework für Linux-Gäste ab macOS 11 (auch Intel)
    targets: [
        .executableTarget(name: "mpkg-vm")
    ]
)
