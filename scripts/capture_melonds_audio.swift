#!/usr/bin/env swift

import AppKit
import AVFoundation
import Foundation
import ScreenCaptureKit

private let defaultBundleID = "net.kuribo64.melonDS"

private struct Options {
    var output: URL?
    var duration = 5.0
    var bundleID = defaultBundleID
    var titleContains: String?
    var force = false
}

private enum CaptureError: LocalizedError {
    case badArguments(String)
    case unsupportedOS
    case outputExists(String)
    case windowNotFound(String)
    case writer(String)
    case noAudio

    var errorDescription: String? {
        switch self {
        case .badArguments(let message): return message
        case .unsupportedOS: return "ScreenCaptureKit audio capture requires macOS 13 or newer."
        case .outputExists(let path): return "Output already exists: \(path) (pass --force to replace it)."
        case .windowNotFound(let bundleID): return "No on-screen window found for \(bundleID). Start melonDS first."
        case .writer(let message): return "Audio writer failed: \(message)"
        case .noAudio: return "No audio samples were received from melonDS."
        }
    }
}

private func usage() -> String {
    """
    Capture melonDS application audio without a virtual audio device.

    Usage:
      xcrun swift scripts/capture_melonds_audio.swift \\
        --duration SECONDS --output FILE.m4a [--force]

    Options:
      -o, --output FILE          Destination M4A file (required)
      -d, --duration SECONDS     Capture duration; default: 5
          --title-contains TEXT  Select a melonDS window by title substring
          --bundle-id ID         Target bundle ID; default: \(defaultBundleID)
      -f, --force                Replace an existing output file
      -h, --help                 Show this help
    """
}

private func parseOptions() throws -> Options {
    var options = Options()
    var index = 1
    let arguments = CommandLine.arguments

    func value(after flag: String) throws -> String {
        guard index + 1 < arguments.count else {
            throw CaptureError.badArguments("Missing value after \(flag).")
        }
        index += 1
        return arguments[index]
    }

    while index < arguments.count {
        let argument = arguments[index]
        switch argument {
        case "-o", "--output":
            options.output = URL(fileURLWithPath: try value(after: argument)).standardizedFileURL
        case "-d", "--duration":
            let raw = try value(after: argument)
            guard let duration = Double(raw), duration > 0, duration.isFinite else {
                throw CaptureError.badArguments("Invalid duration: \(raw)")
            }
            options.duration = duration
        case "--bundle-id":
            options.bundleID = try value(after: argument)
        case "--title-contains":
            options.titleContains = try value(after: argument)
        case "-f", "--force":
            options.force = true
        case "-h", "--help":
            print(usage())
            exit(EXIT_SUCCESS)
        default:
            throw CaptureError.badArguments("Unknown argument: \(argument)\n\n\(usage())")
        }
        index += 1
    }

    guard let output = options.output else {
        throw CaptureError.badArguments("--output is required.\n\n\(usage())")
    }
    guard output.pathExtension.lowercased() == "m4a" else {
        throw CaptureError.badArguments("The output filename must end in .m4a.")
    }
    return options
}

@available(macOS 13.0, *)
private final class AudioSink: NSObject, SCStreamOutput {
    private let writer: AVAssetWriter
    private let input: AVAssetWriterInput
    private let writerQueue = DispatchQueue(label: "net.zeroescape.melonds-audio-writer")
    private var startedSession = false
    private(set) var sampleBufferCount = 0

    init(outputURL: URL) throws {
        do {
            writer = try AVAssetWriter(outputURL: outputURL, fileType: .m4a)
        } catch {
            throw CaptureError.writer(error.localizedDescription)
        }

        input = AVAssetWriterInput(
            mediaType: .audio,
            outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: 48_000,
                AVNumberOfChannelsKey: 2,
                AVEncoderBitRateKey: 192_000,
            ]
        )
        input.expectsMediaDataInRealTime = true

        guard writer.canAdd(input) else {
            throw CaptureError.writer("AVAssetWriter rejected its audio input.")
        }
        writer.add(input)
        guard writer.startWriting() else {
            throw CaptureError.writer(writer.error?.localizedDescription ?? "startWriting() failed")
        }
    }

    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of type: SCStreamOutputType
    ) {
        guard type == .audio, sampleBuffer.isValid else { return }

        writerQueue.sync {
            if !startedSession {
                writer.startSession(atSourceTime: sampleBuffer.presentationTimeStamp)
                startedSession = true
            }
            guard input.isReadyForMoreMediaData else { return }
            if input.append(sampleBuffer) {
                sampleBufferCount += 1
            } else {
                fputs("warning: AVAssetWriterInput.append() failed\n", stderr)
            }
        }
    }

    func finish() async throws {
        let receivedAudio = writerQueue.sync { () -> Bool in
            guard startedSession, sampleBufferCount > 0 else { return false }
            input.markAsFinished()
            return true
        }

        guard receivedAudio else {
            writer.cancelWriting()
            throw CaptureError.noAudio
        }

        await writer.finishWriting()
        guard writer.status == .completed else {
            throw CaptureError.writer(writer.error?.localizedDescription ?? "finishWriting() failed")
        }
    }
}

@available(macOS 13.0, *)
private func capture(options: Options) async throws {
    guard let output = options.output else { preconditionFailure("validated by parseOptions") }
    let fileManager = FileManager.default
    if fileManager.fileExists(atPath: output.path) {
        guard options.force else { throw CaptureError.outputExists(output.path) }
        try fileManager.removeItem(at: output)
    }

    let parent = output.deletingLastPathComponent()
    guard fileManager.fileExists(atPath: parent.path) else {
        throw CaptureError.badArguments("Output directory does not exist: \(parent.path)")
    }

    // SCContentFilter touches SkyLight; initializing NSApplication first avoids
    // CGS_REQUIRE_INIT when this file is run as a command-line Swift script.
    await MainActor.run { _ = NSApplication.shared }

    let content = try await SCShareableContent.excludingDesktopWindows(
        false,
        onScreenWindowsOnly: true
    )
    let candidates = content.windows.filter { window in
        guard window.owningApplication?.bundleIdentifier == options.bundleID else { return false }
        guard let title = options.titleContains else { return true }
        return window.title?.localizedCaseInsensitiveContains(title) == true
    }
    guard let window = candidates.max(by: {
        ($0.frame.width * $0.frame.height) < ($1.frame.width * $1.frame.height)
    }) else {
        throw CaptureError.windowNotFound(options.bundleID)
    }

    let filter = SCContentFilter(desktopIndependentWindow: window)
    let configuration = SCStreamConfiguration()
    configuration.capturesAudio = true
    configuration.sampleRate = 48_000
    configuration.channelCount = 2
    configuration.excludesCurrentProcessAudio = true

    let sink = try AudioSink(outputURL: output)
    let stream = SCStream(filter: filter, configuration: configuration, delegate: nil)
    let audioQueue = DispatchQueue(label: "net.zeroescape.melonds-audio-capture")
    try stream.addStreamOutput(sink, type: .audio, sampleHandlerQueue: audioQueue)

    try await stream.startCapture()
    try await Task.sleep(for: .seconds(options.duration))
    try await stream.stopCapture()
    try await sink.finish()

    print("window_id=\(window.windowID)")
    print("window_title=\(window.title ?? "")")
    print("duration_requested=\(options.duration)")
    print("sample_buffers=\(sink.sampleBufferCount)")
    print("output=\(output.path)")
}

do {
    let options = try parseOptions()
    guard #available(macOS 13.0, *) else { throw CaptureError.unsupportedOS }
    try await capture(options: options)
} catch {
    fputs("error: \(error.localizedDescription)\n", stderr)
    exit(EXIT_FAILURE)
}
