// memory-watchdog-notify: posts notifications as "Memory Watchdog" so they carry
// the app's icon. Lives inside Memory Watchdog.app; the Rust daemon calls it.
//
//   memory-watchdog-notify TITLE BODY   post a notification (exit 3 if not allowed)
//   memory-watchdog-notify --authorize  ask for permission (run via `open` once)
//   memory-watchdog-notify --status     print the permission state (exit 0 if allowed)
import Foundation
import UserNotifications

let center = UNUserNotificationCenter.current()
let args = Array(CommandLine.arguments.dropFirst())

func done(_ code: Int32, _ msg: String? = nil) -> Never {
    if let msg { print(msg) }
    exit(code)
}

func statusName(_ s: UNAuthorizationStatus) -> String {
    switch s {
    case .authorized: return "authorized"
    case .denied: return "denied"
    case .notDetermined: return "not-determined"
    case .provisional: return "provisional"
    case .ephemeral: return "ephemeral"
    @unknown default: return "unknown"
    }
}

func withSettings(_ body: @escaping (UNNotificationSettings) -> Void) {
    center.getNotificationSettings { body($0) }
}

var timeout: TimeInterval = 10

switch args.first {
case "--status":
    withSettings { s in done(s.authorizationStatus == .authorized ? 0 : 1, statusName(s.authorizationStatus)) }
case "--authorize":
    timeout = 300 // the user has to click Allow
    center.requestAuthorization(options: [.alert, .sound]) { granted, error in
        if let error { done(1, "error: \(error.localizedDescription)") }
        done(granted ? 0 : 1, granted ? "authorized" : "denied")
    }
case .some(let title) where args.count >= 2 && !title.hasPrefix("--"):
    withSettings { s in
        guard s.authorizationStatus == .authorized || s.authorizationStatus == .provisional else {
            done(3, statusName(s.authorizationStatus))
        }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = args[1]
        content.sound = .default
        let req = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        center.add(req) { error in
            if let error { done(1, "error: \(error.localizedDescription)") }
            done(0)
        }
    }
default:
    done(2, "usage: memory-watchdog-notify TITLE BODY | --authorize | --status")
}

// Wait for the callback; never hang the caller.
DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { done(4, "timeout") }
RunLoop.main.run()
