#!/usr/bin/env swift

import AppKit
import ApplicationServices
import CoreGraphics
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

    var errorDescription: String? {
        switch self {
        case .badArguments(let message): return message
        case .applicationNotRunning: return "melonDS is not running."
        case .windowNotFound: return "No on-screen melonDS emulator window was found."
        case .eventCreationFailed: return "CoreGraphics could not create an input event."
        }
    }
}

private func usage() -> String {
    """
    Send deterministic CoreGraphics input to the largest melonDS window.

    Usage:
      xcrun swift scripts/melonds_input.swift info [--timeout SECONDS]
      xcrun swift scripts/melonds_input.swift touch SCREEN X Y [--hold SECONDS] [--timeout SECONDS]
      xcrun swift scripts/melonds_input.swift key KEYCODE [--hold SECONDS] [MODIFIERS]

    Commands:
      info                    Print the selected window and calculated DS geometry
      touch top|bottom X Y    Touch DS pixel coordinates (X: 0...255, Y: 0...191)
      key KEYCODE             Send a macOS virtual key code (decimal or 0x-prefixed)

    Modifiers for key:
      --shift --control --option --command

    Common macOS key codes:
      Return=36  Space=49  Escape=53  F1=122  F2=120  F9=101
    """
}

private func optionValue(_ name: String, in arguments: [String], default fallback: Double) throws -> Double {
    guard let index = arguments.firstIndex(of: name) else { return fallback }
    guard index + 1 < arguments.count, let value = Double(arguments[index + 1]), value >= 0, value.isFinite else {
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
        // The normal melonDS layout is two 256x192 screens stacked vertically.
        // Any excess height is native title-bar chrome above the rendered area.
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

private func activate(_ application: NSRunningApplication) {
    application.activate(options: [])
    Thread.sleep(forTimeInterval: 0.12)
}

private func postMouseClick(at point: CGPoint, hold: Double) throws {
    guard let move = CGEvent(
        mouseEventSource: nil,
        mouseType: .mouseMoved,
        mouseCursorPosition: point,
        mouseButton: .left
    ), let down = CGEvent(
        mouseEventSource: nil,
        mouseType: .leftMouseDown,
        mouseCursorPosition: point,
        mouseButton: .left
    ), let up = CGEvent(
        mouseEventSource: nil,
        mouseType: .leftMouseUp,
        mouseCursorPosition: point,
        mouseButton: .left
    ) else { throw InputError.eventCreationFailed }

    move.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.03)
    down.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: hold)
    up.post(tap: .cghidEventTap)
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

private func postKey(code: CGKeyCode, flags: CGEventFlags, hold: Double) throws {
    guard let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false) else {
        throw InputError.eventCreationFailed
    }
    down.flags = flags
    up.flags = flags
    down.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: hold)
    up.post(tap: .cghidEventTap)
}

do {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard let command = arguments.first, command != "-h", command != "--help" else {
        print(usage())
        exit(EXIT_SUCCESS)
    }

    let hold = try optionValue("--hold", in: arguments, default: 0.06)
    let timeout = try optionValue("--timeout", in: arguments, default: 5.0)

    switch command {
    case "info":
        let window = try findWindow(timeout: timeout)
        let geometry = DSGeometry(windowBounds: window.bounds)
        print("window_id=\(window.id)")
        print("pid=\(window.application.processIdentifier)")
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
        let window = try findWindow(timeout: timeout)
        let point = try DSGeometry(windowBounds: window.bounds).point(screen: arguments[1], x: x, y: y)
        activate(window.application)
        try postMouseClick(at: point, hold: hold)
        print("window_id=\(window.id)")
        print("click=\(point.x),\(point.y)")

    case "key":
        guard arguments.count >= 2 else {
            throw InputError.badArguments("key requires a virtual KEYCODE.\n\n\(usage())")
        }
        let window = try findWindow(timeout: timeout)
        var flags: CGEventFlags = []
        if arguments.contains("--shift") { flags.insert(.maskShift) }
        if arguments.contains("--control") { flags.insert(.maskControl) }
        if arguments.contains("--option") { flags.insert(.maskAlternate) }
        if arguments.contains("--command") { flags.insert(.maskCommand) }
        activate(window.application)
        try postKey(code: parseKeyCode(arguments[1]), flags: flags, hold: hold)
        print("window_id=\(window.id)")
        print("keycode=\(arguments[1])")

    default:
        throw InputError.badArguments("Unknown command: \(command)\n\n\(usage())")
    }
} catch {
    if !AXIsProcessTrusted() {
        fputs("hint: allow your terminal/Codex host in Privacy & Security > Accessibility.\n", stderr)
    }
    fputs("error: \(error.localizedDescription)\n", stderr)
    exit(EXIT_FAILURE)
}
