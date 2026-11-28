// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "mpkg-vm",
    platforms: [.macOS(.v12)], // Linux boot from 12, EFI boot from 13; current toolchains no longer build for 11
    targets: [
        .executableTarget(name: "mpkg-vm")
    ]
)
