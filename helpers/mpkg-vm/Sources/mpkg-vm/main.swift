// mpkg-vm – kleiner Helfer, der die Debian-VM über Virtualization.framework betreibt.
// Wird vom Rust-Kern (src/backend/linuxvm.rs) im Hintergrund gestartet; Download,
// Provisionierung (cloud-init), Status und `exec` (per ssh) erledigt der Rust-Kern.
//
//   mpkg-vm run <vm-dir> <share-dir>
//
// <vm-dir> enthält disk.raw, seed.iso, mac (MAC-Adresse) und efi.vars (wird angelegt).
// <share-dir> wird per virtiofs mit dem Tag "home" in den Gast gereicht.
// Läuft im Vordergrund, bis der Gast herunterfährt oder SIGTERM kommt.
//
// Die Binary braucht das Entitlement com.apple.security.virtualization:
//   codesign --sign - --entitlements mpkg-vm.entitlements --force .build/release/mpkg-vm

import Foundation
import Virtualization

// shortcut: feste Ressourcen, bei Bedarf später über Argumente konfigurierbar machen.
let cpuCount = 2
let memoryBytes: UInt64 = 2 * 1024 * 1024 * 1024

func log(_ message: String) {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
}

func fail(_ message: String) -> Never {
    log("mpkg-vm: " + message)
    exit(1)
}

func makeConfiguration(vmDir: URL, share: URL) throws -> VZVirtualMachineConfiguration {
    let config = VZVirtualMachineConfiguration()
    config.cpuCount = cpuCount
    config.memorySize = memoryBytes

    let varsURL = vmDir.appendingPathComponent("efi.vars")
    let loader = VZEFIBootLoader()
    loader.variableStore = FileManager.default.fileExists(atPath: varsURL.path)
        ? VZEFIVariableStore(url: varsURL)
        : try VZEFIVariableStore(creatingVariableStoreAt: varsURL)
    config.bootLoader = loader
    config.platform = VZGenericPlatformConfiguration()

    let disk = try VZDiskImageStorageDeviceAttachment(
        url: vmDir.appendingPathComponent("disk.raw"), readOnly: false)
    let seed = try VZDiskImageStorageDeviceAttachment(
        url: vmDir.appendingPathComponent("seed.iso"), readOnly: true)
    config.storageDevices = [
        VZVirtioBlockDeviceConfiguration(attachment: disk),
        VZVirtioBlockDeviceConfiguration(attachment: seed),
    ]

    let net = VZVirtioNetworkDeviceConfiguration()
    net.attachment = VZNATNetworkDeviceAttachment()
    let macText = try String(contentsOf: vmDir.appendingPathComponent("mac"), encoding: .utf8)
    guard let mac = VZMACAddress(string: macText.trimmingCharacters(in: .whitespacesAndNewlines)) else {
        throw NSError(domain: "mpkg-vm", code: 1, userInfo: [NSLocalizedDescriptionKey: "ungültige MAC-Adresse"])
    }
    net.macAddress = mac
    config.networkDevices = [net]

    let fs = VZVirtioFileSystemDeviceConfiguration(tag: "home")
    fs.share = VZSingleDirectoryShare(directory: VZSharedDirectory(url: share, readOnly: false))
    config.directorySharingDevices = [fs]

    // Serielle Konsole nach stdout – der Rust-Kern leitet sie in vm-dir/console.log.
    let serial = VZVirtioConsoleDeviceSerialPortConfiguration()
    serial.attachment = VZFileHandleSerialPortAttachment(
        fileHandleForReading: nil, fileHandleForWriting: FileHandle.standardOutput)
    config.serialPorts = [serial]

    config.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
    config.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]

    try config.validate()
    return config
}

final class Runner: NSObject, VZVirtualMachineDelegate {
    let vm: VZVirtualMachine
    var signalSource: DispatchSourceSignal?

    init(config: VZVirtualMachineConfiguration) {
        vm = VZVirtualMachine(configuration: config)
        super.init()
        vm.delegate = self
    }

    func start() {
        // SIGTERM (`mpkg vm stop`): Gast sauber herunterfahren, nach 20 s hart stoppen.
        signal(SIGTERM, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        source.setEventHandler { [weak self] in self?.shutdown() }
        source.resume()
        signalSource = source

        vm.start { result in
            if case .failure(let error) = result {
                fail("Start fehlgeschlagen: \(error.localizedDescription)")
            }
            log("VM läuft")
        }
    }

    func shutdown() {
        if vm.canRequestStop {
            try? vm.requestStop()
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 20) { [weak self] in
            self?.vm.stop { _ in exit(0) }
        }
    }

    func guestDidStop(_ virtualMachine: VZVirtualMachine) {
        log("Gast heruntergefahren")
        exit(0)
    }

    func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: Error) {
        fail("VM gestoppt: \(error.localizedDescription)")
    }
}

let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 3, args[0] == "run" else {
    fail("Verwendung: mpkg-vm run <vm-dir> <share-dir>")
}

let vmDir = URL(fileURLWithPath: args[1], isDirectory: true)
let share = URL(fileURLWithPath: args[2], isDirectory: true)
let runner: Runner
do {
    runner = Runner(config: try makeConfiguration(vmDir: vmDir, share: share))
} catch {
    fail("Konfiguration ungültig: \(error.localizedDescription)")
}
// VZVirtualMachine muss auf der Queue bedient werden, auf der es erzeugt wurde (main).
runner.start()
dispatchMain()
