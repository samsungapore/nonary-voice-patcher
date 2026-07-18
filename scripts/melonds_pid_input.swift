#!/usr/bin/env swift

import AppKit
import ApplicationServices
import Carbon
import Foundation

private let bundleID = "net.kuribo64.melonDS"

private struct MelonWindow {
    let application: NSRunningApplication
    let id: CGWindowID
    let title: String
    let bounds: CGRect
}

private enum InputError: LocalizedError {
    case badArguments(String)
    case applicationNotRunning
    case windowNotFound
    case eventCreationFailed
    case keyboardLayoutUnavailable
    case characterNotFound(String)

    var errorDescription: String? {
        switch self {
        case .badArguments(let message): return message
        case .applicationNotRunning: return "melonDS is not running."
        case .windowNotFound: return "No on-screen melonDS emulator window was found."
        case .eventCreationFailed: return "CoreGraphics could not create an input event."
        case .keyboardLayoutUnavailable: return "The active macOS keyboard layout cannot be inspected."
        case .characterNotFound(let character):
            return "No unmodified key in the active layout produces '\(character)'."
        }
    }
}

private func usage() -> String {
    """
    Send input directly to the melonDS process with CGEvent.postToPid.

    Usage:
      xcrun swift scripts/melonds_pid_input.swift info [--timeout SECONDS]
      xcrun swift scripts/melonds_pid_input.swift touch top|bottom X Y [--hold SECONDS]
      xcrun swift scripts/melonds_pid_input.swift char CHARACTER [--hold SECONDS]
      xcrun swift scripts/melonds_pid_input.swift keycode KEYCODE [--hold SECONDS]

    `char` resolves the physical macOS key from the active keyboard layout.
    This matters on AZERTY: melonDS' Qt mapping A=Z needs keycode 13, not 6.
    """
}

private func optionValue(_ name: String, in arguments: [String], default fallback: Double) throws -> Double {
    guard let index = arguments.firstIndex(of: name) else { return fallback }
    guard index + 1 < arguments.count,
          let value = Double(arguments[index + 1]),
          value >= 0,
          value.isFinite else {
        throw InputError.badArguments("Invalid or missing value after \(name).")
    }
    return value
}

private func findWindow(timeout: Double) throws -> MelonWindow {
    let deadline = Date().addingTimeInterval(timeout)
    repeat {
        let applications = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
        let byPID = Dictionary(uniqueKeysWithValues: applications.map { ($0.processIdentifier, $0) })
        if !byPID.isEmpty,
           let rawWindows = CGWindowListCopyWindowInfo(
               [.optionOnScreenOnly, .excludeDesktopElements],
               kCGNullWindowID
           ) as? [[String: Any]] {
            let windows: [MelonWindow] = rawWindows.compactMap { raw in
                guard let pidNumber = raw[kCGWindowOwnerPID as String] as? NSNumber,
                      let application = byPID[pid_t(pidNumber.int32Value)],
                      let layerNumber = raw[kCGWindowLayer as String] as? NSNumber,
                      layerNumber.intValue == 0,
                      let boundsValue = raw[kCGWindowBounds as String],
                      let idNumber = raw[kCGWindowNumber as String] as? NSNumber,
                      let bounds = CGRect(dictionaryRepresentation: boundsValue as! CFDictionary),
                      bounds.width > 0,
                      bounds.height > 0 else { return nil }
                return MelonWindow(
                    application: application,
                    id: CGWindowID(idNumber.uint32Value),
                    title: raw[kCGWindowName as String] as? String ?? "",
                    bounds: bounds
                )
            }
            if let window = windows.max(by: {
                ($0.bounds.width * $0.bounds.height) < ($1.bounds.width * $1.bounds.height)
            }) {
                return window
            }
        }
        if Date() < deadline { Thread.sleep(forTimeInterval: 0.1) }
    } while Date() < deadline

    if NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).isEmpty {
        throw InputError.applicationNotRunning
    }
    throw InputError.windowNotFound
}

private struct DSGeometry {
    let scale: CGFloat
    let contentOrigin: CGPoint

    init(windowBounds: CGRect) {
        // The normal layout is two 256x192 screens stacked vertically. The
        // extra window height is the native title bar above the render panel.
        scale = min(windowBounds.width / 256.0, windowBounds.height / 384.0)
        let contentWidth = 256.0 * scale
        let contentHeight = 384.0 * scale
        contentOrigin = CGPoint(
            x: windowBounds.minX + (windowBounds.width - contentWidth) / 2.0,
            y: windowBounds.maxY - contentHeight
        )
    }

    func point(screen: String, x: Double, y: Double) throws -> CGPoint {
        guard screen == "top" || screen == "bottom" else {
            throw InputError.badArguments("SCREEN must be top or bottom.")
        }
        guard (0.0...255.0).contains(x), (0.0...191.0).contains(y) else {
            throw InputError.badArguments("DS coordinates out of range (X: 0...255, Y: 0...191).")
        }
        let screenOffset = screen == "bottom" ? 192.0 * scale : 0.0
        return CGPoint(
            x: contentOrigin.x + CGFloat(x) * scale,
            y: contentOrigin.y + screenOffset + CGFloat(y) * scale
        )
    }
}

private func parseKeyCode(_ raw: String) throws -> CGKeyCode {
    let value: UInt64?
    if raw.lowercased().hasPrefix("0x") {
        value = UInt64(raw.dropFirst(2), radix: 16)
    } else {
        value = UInt64(raw)
    }
    guard let value, value <= UInt64(UInt16.max) else {
        throw InputError.badArguments("Invalid key code: \(raw)")
    }
    return CGKeyCode(value)
}

private func unmodifiedText(for keyCode: CGKeyCode) throws -> String {
    let source = TISCopyCurrentKeyboardLayoutInputSource().takeRetainedValue()
    guard let raw = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) else {
        throw InputError.keyboardLayoutUnavailable
    }
    let data = Unmanaged<CFData>.fromOpaque(raw).takeUnretainedValue()
    guard let bytes = CFDataGetBytePtr(data) else {
        throw InputError.keyboardLayoutUnavailable
    }
    let layout = UnsafeRawPointer(bytes).assumingMemoryBound(to: UCKeyboardLayout.self)
    var deadKeyState: UInt32 = 0
    var characters = [UniChar](repeating: 0, count: 8)
    var length = 0
    let status = UCKeyTranslate(
        layout,
        keyCode,
        UInt16(kUCKeyActionDown),
        0,
        UInt32(LMGetKbdType()),
        UInt32(kUCKeyTranslateNoDeadKeysBit),
        &deadKeyState,
        characters.count,
        &length,
        &characters
    )
    guard status == noErr else { throw InputError.keyboardLayoutUnavailable }
    return String(utf16CodeUnits: characters, count: length)
}

private func keyCode(for character: String) throws -> CGKeyCode {
    guard character.count == 1 else {
        throw InputError.badArguments("char requires exactly one character.")
    }
    for code in CGKeyCode(0)...CGKeyCode(127) {
        if try unmodifiedText(for: code).localizedCaseInsensitiveCompare(character) == .orderedSame {
            return code
        }
    }
    throw InputError.characterNotFound(character)
}

private func activate(_ application: NSRunningApplication) {
    application.activate(options: [])
    let appElement = AXUIElementCreateApplication(application.processIdentifier)
    AXUIElementPerformAction(appElement, kAXRaiseAction as CFString)
    Thread.sleep(forTimeInterval: 0.08)
}

private func postMouseClick(to pid: pid_t, at point: CGPoint, hold: Double) throws {
    let source = CGEventSource(stateID: .privateState)
    guard let move = CGEvent(
        mouseEventSource: source,
        mouseType: .mouseMoved,
        mouseCursorPosition: point,
        mouseButton: .left
    ), let down = CGEvent(
        mouseEventSource: source,
        mouseType: .leftMouseDown,
        mouseCursorPosition: point,
        mouseButton: .left
    ), let up = CGEvent(
        mouseEventSource: source,
        mouseType: .leftMouseUp,
        mouseCursorPosition: point,
        mouseButton: .left
    ) else { throw InputError.eventCreationFailed }

    move.postToPid(pid)
    Thread.sleep(forTimeInterval: 0.03)
    down.postToPid(pid)
    Thread.sleep(forTimeInterval: hold)
    up.postToPid(pid)
}

private func postKey(to pid: pid_t, code: CGKeyCode, hold: Double) throws {
    let source = CGEventSource(stateID: .privateState)
    guard let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false) else {
        throw InputError.eventCreationFailed
    }
    down.postToPid(pid)
    Thread.sleep(forTimeInterval: hold)
    up.postToPid(pid)
}

do {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard let command = arguments.first, command != "-h", command != "--help" else {
        print(usage())
        exit(EXIT_SUCCESS)
    }

    let hold = try optionValue("--hold", in: arguments, default: 0.08)
    let timeout = try optionValue("--timeout", in: arguments, default: 5.0)
    let window = try findWindow(timeout: timeout)
    let pid = window.application.processIdentifier

    switch command {
    case "info":
        let geometry = DSGeometry(windowBounds: window.bounds)
        print("window_id=\(window.id)")
        print("pid=\(pid)")
        print("title=\(window.title)")
        print("bounds=\(Int(window.bounds.minX)),\(Int(window.bounds.minY)),\(Int(window.bounds.width)),\(Int(window.bounds.height))")
        print("scale=\(geometry.scale)")
        print("content_origin=\(geometry.contentOrigin.x),\(geometry.contentOrigin.y)")

    case "touch":
        guard arguments.count >= 4,
              let x = Double(arguments[2]),
              let y = Double(arguments[3]) else {
            throw InputError.badArguments("touch requires: SCREEN X Y\n\n\(usage())")
        }
        let point = try DSGeometry(windowBounds: window.bounds).point(
            screen: arguments[1], x: x, y: y
        )
        activate(window.application)
        try postMouseClick(to: pid, at: point, hold: hold)
        print("window_id=\(window.id)")
        print("pid=\(pid)")
        print("click=\(point.x),\(point.y)")

    case "char":
        guard arguments.count >= 2 else {
            throw InputError.badArguments("char requires one character.\n\n\(usage())")
        }
        let code = try keyCode(for: arguments[1])
        activate(window.application)
        try postKey(to: pid, code: code, hold: hold)
        print("window_id=\(window.id)")
        print("pid=\(pid)")
        print("character=\(arguments[1])")
        print("keycode=\(code)")

    case "keycode":
        guard arguments.count >= 2 else {
            throw InputError.badArguments("keycode requires a macOS virtual key code.\n\n\(usage())")
        }
        let code = try parseKeyCode(arguments[1])
        activate(window.application)
        try postKey(to: pid, code: code, hold: hold)
        print("window_id=\(window.id)")
        print("pid=\(pid)")
        print("keycode=\(code)")

    default:
        throw InputError.badArguments("Unknown command: \(command)\n\n\(usage())")
    }
} catch {
    if !AXIsProcessTrusted() {
        fputs("hint: allow the terminal/Codex host in Privacy & Security > Accessibility.\n", stderr)
    }
    fputs("error: \(error.localizedDescription)\n", stderr)
    exit(EXIT_FAILURE)
}
