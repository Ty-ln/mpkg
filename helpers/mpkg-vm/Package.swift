// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "mpkg-vm",
    platforms: [.macOS(.v11)], // Linux-Boot ab macOS 11, EFI-Boot ab 13, Home-Freigabe ab 12
    targets: [
        .executableTarget(name: "mpkg-vm")
    ]
)
