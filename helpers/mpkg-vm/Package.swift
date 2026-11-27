// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "mpkg-vm",
    platforms: [.macOS(.v13)], // EFI-Boot für Linux-Gäste gibt es ab macOS 13 (Intel und Apple Silicon)
    targets: [
        .executableTarget(name: "mpkg-vm")
    ]
)
