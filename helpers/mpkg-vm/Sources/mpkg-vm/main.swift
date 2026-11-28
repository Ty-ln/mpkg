// mpkg-vm – small helper that runs the Debian VM via Virtualization.framework.
// Started in the background by the Rust core (src/vm.rs); download, provisioning
// (cloud-init), status and `exec` (over ssh) are handled by the Rust core.
//
//   mpkg-vm run <vm-dir> <share-dir>
//
// <vm-dir> contains disk.raw, seed.iso, mac (MAC address) and efi.vars (created if missing).
// If vmlinuz, initrd and cmdline are present, the VM boots via VZLinuxBootLoader (macOS 12),
// otherwise via EFI (macOS 13+).
// Idle mode: the Rust core touches <vm-dir>/last-used on every call. If that is longer ago
// than idleSeconds and no ssh session with the VM key is running, the VM shuts down.
// <share-dir> is passed to the guest via virtiofs with the tag "home".
// Runs in the foreground until the guest shuts down or SIGTERM arrives.
//
// The binary needs the entitlement com.apple.security.virtualization:
//   codesign --sign - --entitlements mpkg-vm.entitlements --force .build/release/mpkg-vm

import Foundation
import Virtualization

// shortcut: fixed resources, make them configurable via arguments if needed.
let cpuCount = 2
let memoryBytes: UInt64 = 2 * 1024 * 1024 * 1024
// MPKG_VM_IDLE_SECONDS overrides the value (for tests).
let idleSeconds: TimeInterval = ProcessInfo.processInfo.environment["MPKG_VM_IDLE_SECONDS"].flatMap(TimeInterval.init) ?? 10 * 60

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

    let kernelURL = vmDir.appendingPathComponent("vmlinuz")
    if FileManager.default.fileExists(atPath: kernelURL.path) {
        let loader = VZLinuxBootLoader(kernelURL: kernelURL)
        loader.initialRamdiskURL = vmDir.appendingPathComponent("initrd")
        loader.commandLine = try String(contentsOf: vmDir.appendingPathComponent("cmdline"), encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        config.bootLoader = loader
    } else if #available(macOS 13, *) {
        let varsURL = vmDir.appendingPathComponent("efi.vars")
        let loader = VZEFIBootLoader()
        loader.variableStore = FileManager.default.fileExists(atPath: varsURL.path)
            ? VZEFIVariableStore(url: varsURL)
            : try VZEFIVariableStore(creatingVariableStoreAt: varsURL)
        config.bootLoader = loader
        config.platform = VZGenericPlatformConfiguration()
    } else {
        throw NSError(domain: "mpkg-vm", code: 2, userInfo: [NSLocalizedDescriptionKey: "vmlinuz is missing (required on macOS 12)"])
    }

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
        throw NSError(domain: "mpkg-vm", code: 1, userInfo: [NSLocalizedDescriptionKey: "invalid MAC address"])
    }
    net.macAddress = mac
    config.networkDevices = [net]

    let fs = VZVirtioFileSystemDeviceConfiguration(tag: "home")
    fs.share = VZSingleDirectoryShare(directory: VZSharedDirectory(url: share, readOnly: false))
    config.directorySharingDevices = [fs]

    // Serial console to stdout – the Rust core redirects it to vm-dir/console.log.
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
    let vmDir: URL
    let startedAt = Date()
    var signalSource: DispatchSourceSignal?
    var idleTimer: DispatchSourceTimer?

    init(config: VZVirtualMachineConfiguration, vmDir: URL) {
        vm = VZVirtualMachine(configuration: config)
        self.vmDir = vmDir
        super.init()
        vm.delegate = self
    }

    /// Last use according to last-used, otherwise the start time.
    func lastUsed() -> Date {
        let path = vmDir.appendingPathComponent("last-used").path
        let modified = (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
        return max(modified ?? startedAt, startedAt)
    }

    /// Is an ssh session into the VM still running (e.g. an open htop)?
    func hasSession() -> Bool {
        let pgrep = Process()
        pgrep.executableURL = URL(fileURLWithPath: "/usr/bin/pgrep")
        pgrep.arguments = ["-f", vmDir.appendingPathComponent("id_ed25519").path]
        pgrep.standardOutput = FileHandle.nullDevice
        guard (try? pgrep.run()) != nil else { return true } // when in doubt, do not stop
        pgrep.waitUntilExit()
        return pgrep.terminationStatus == 0
    }

    func checkIdle() {
        if Date().timeIntervalSince(lastUsed()) > idleSeconds && !hasSession() {
            log("Idle: \(Int(idleSeconds)) s without use – shutting down")
            idleTimer?.cancel()
            shutdown()
        }
    }

    func start() {
        // SIGTERM (`mpkg vm stop`): shut the guest down cleanly, hard stop after 20 s.
        signal(SIGTERM, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        source.setEventHandler { [weak self] in self?.shutdown() }
        source.resume()
        signalSource = source

        vm.start { result in
            if case .failure(let error) = result {
                fail("start failed: \(error.localizedDescription)")
            }
            log("VM running")
        }
        // DispatchSource instead of Timer: dispatchMain() runs no RunLoop.
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now() + 60, repeating: 60)
        timer.setEventHandler { [weak self] in self?.checkIdle() }
        timer.resume()
        idleTimer = timer
    }

    /// Shut down cleanly via the guest: with the Linux boot loader (macOS 12) the guest does
    /// not react to requestStop. The Rust core stores the IP in <vm-dir>/ip.
    func poweroffViaSSH() {
        guard let ip = try? String(contentsOf: vmDir.appendingPathComponent("ip"), encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines), !ip.isEmpty else { return }
        let ssh = Process()
        ssh.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        ssh.arguments = [
            "-i", vmDir.appendingPathComponent("id_ed25519").path,
            "-o", "UserKnownHostsFile=" + vmDir.appendingPathComponent("known_hosts").path,
            "-o", "StrictHostKeyChecking=yes", "-o", "BatchMode=yes", "-o", "ConnectTimeout=3",
            "mpkg@" + ip, "sudo systemctl poweroff",
        ]
        ssh.standardOutput = FileHandle.nullDevice
        ssh.standardError = FileHandle.nullDevice
        try? ssh.run() // do not wait: guestDidStop ends the process
    }

    func shutdown() {
        poweroffViaSSH()
        if vm.canRequestStop {
            try? vm.requestStop()
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 20) { [weak self] in
            self?.vm.stop { _ in exit(0) }
        }
    }

    func guestDidStop(_ virtualMachine: VZVirtualMachine) {
        log("Guest shut down")
        exit(0)
    }

    func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: Error) {
        fail("VM stopped: \(error.localizedDescription)")
    }
}

let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 3, args[0] == "run" else {
    fail("usage: mpkg-vm run <vm-dir> <share-dir>")
}

let vmDir = URL(fileURLWithPath: args[1], isDirectory: true)
let share = URL(fileURLWithPath: args[2], isDirectory: true)
let runner: Runner
do {
    runner = Runner(config: try makeConfiguration(vmDir: vmDir, share: share), vmDir: vmDir)
} catch {
    fail("invalid configuration: \(error.localizedDescription)")
}
// VZVirtualMachine must be used on the queue it was created on (main).
runner.start()
dispatchMain()
