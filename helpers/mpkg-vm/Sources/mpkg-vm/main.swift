// mpkg-vm – kleiner Helfer, der die Debian-VM über Virtualization.framework steuert.
// Wird vom Rust-Kern (src/backend/linuxvm.rs) aufgerufen. Phase 2 – bisher nur Gerüst.
//
// Protokoll:
//   mpkg-vm status          -> JSON {"state": "running" | "stopped" | "missing"}
//   mpkg-vm start | stop
//   mpkg-vm exec -- <cmd>   -> Befehl im Gast ausführen, Exit-Code durchreichen
//
// Hinweis: Die Binary braucht das Entitlement com.apple.security.virtualization
// (siehe mpkg-vm.entitlements) und muss damit signiert werden:
//   codesign --sign - --entitlements mpkg-vm.entitlements --force .build/release/mpkg-vm

import Foundation
import Virtualization

let args = Array(CommandLine.arguments.dropFirst())

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

switch args.first {
case "status":
    // TODO: prüfen, ob Image und Konfiguration unter ~/.mpkg/vm existieren und die VM läuft.
    print(#"{"state": "missing"}"#)
case "start":
    // TODO: VZVirtualMachineConfiguration mit VZLinuxBootLoader, virtio-Disk,
    //       VZVirtioFileSystemDeviceConfiguration (Home teilen), NAT-Netzwerk.
    fail("start: noch nicht implementiert")
case "stop":
    fail("stop: noch nicht implementiert")
case "exec":
    // TODO: Befehl über vsock (VZVirtioSocketDevice) an einen kleinen Agenten im Gast senden.
    fail("exec: noch nicht implementiert")
default:
    fail("Verwendung: mpkg-vm status | start | stop | exec -- <cmd> …")
}
