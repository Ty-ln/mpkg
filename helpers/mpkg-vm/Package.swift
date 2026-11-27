// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "mpkg-vm",
    platforms: [.macOS(.v12)], // Linux-Boot ab 12, EFI-Boot ab 13; aktuelle Toolchains bauen nicht mehr für 11
    targets: [
        .executableTarget(name: "mpkg-vm")
    ]
)
