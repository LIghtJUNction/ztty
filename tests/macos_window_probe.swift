// Native test helper only. All pixels come from ScreenCaptureKit's WindowServer
// window capture; no ztty rendering buffers or private accessibility APIs.
import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation
import ImageIO
@preconcurrency import ScreenCaptureKit

struct Sample: Decodable {
    let x: Int
    let y: Int
    let rgb: [Int]
}

struct Request: Decodable {
    let action: String
    let pid: Int32?
    let windowID: UInt32?
    let windowTitle: String?
    let png: String?
    let expectedClientPixels: [Int]?
    let targetClientPixels: [Int]?
    let samples: [Sample]?
    let timeout: Double?
    let binary: String?
    let arguments: [String]?

    enum CodingKeys: String, CodingKey {
        case action, pid, png, samples, timeout, binary, arguments
        case windowID = "window_id"
        case windowTitle = "window_title"
        case expectedClientPixels = "expected_client_pixels"
        case targetClientPixels = "target_client_pixels"
    }
}

struct GateError: Error, CustomStringConvertible {
    let description: String
    let blocked: Bool
    init(_ description: String, blocked: Bool = false) {
        self.description = description
        self.blocked = blocked
    }
}

func rectJSON(_ rect: CGRect) -> [Double] {
    [Double(rect.minX), Double(rect.minY), Double(rect.width), Double(rect.height)]
}

// WindowServer may expose menu/auxiliary surfaces under the same app PID.
// Select the explicitly titled terminal, then keep its WindowID pinned.
@MainActor
struct CaptureWindowIdentity {
    let pid: pid_t
    let id: CGWindowID
    let title: String?
    let layer: Int
    let onScreen: Bool

    func matches(pid: pid_t, title: String, windowID: CGWindowID?) -> Bool {
        self.pid == pid && self.title == title
            && layer == NSWindow.Level.normal.rawValue && onScreen
            && (windowID == nil || id == windowID)
    }
}

@MainActor
func uniqueCaptureWindow(_ candidates: [CaptureWindowIdentity], pid: pid_t,
                         title: String, windowID: CGWindowID?) throws -> CGWindowID {
    guard !title.isEmpty else { throw GateError("missing unique terminal window title") }
    let matches = candidates.filter { $0.matches(pid: pid, title: title, windowID: windowID) }
    guard matches.count == 1 else {
        throw GateError("expected one owned terminal window; PID \(pid), title \(title), WindowID \(String(describing: windowID)), matching \(matches.count) of \(candidates.count) candidates")
    }
    return matches[0].id
}

@MainActor
func verifyWindowSelection() throws {
    let title = "ztty-native-selection-fixture"
    let normal = NSWindow.Level.normal.rawValue
    let terminal = CaptureWindowIdentity(pid: 21, id: 1, title: title, layer: normal, onScreen: true)
    let candidates = [
        terminal,
        CaptureWindowIdentity(pid: 21, id: 2, title: "menu", layer: normal + 1, onScreen: true),
        CaptureWindowIdentity(pid: 21, id: 3, title: nil, layer: normal, onScreen: true),
        CaptureWindowIdentity(pid: 21, id: 4, title: title, layer: normal + 1, onScreen: true),
        CaptureWindowIdentity(pid: 21, id: 5, title: title, layer: normal, onScreen: false),
        CaptureWindowIdentity(pid: 21, id: 6, title: "other", layer: normal, onScreen: true),
        CaptureWindowIdentity(pid: 22, id: 7, title: title, layer: normal, onScreen: true),
    ]
    guard try uniqueCaptureWindow(candidates, pid: 21, title: title, windowID: nil) == 1,
          try uniqueCaptureWindow(candidates, pid: 21, title: title, windowID: 1) == 1,
          (try? uniqueCaptureWindow(candidates, pid: 21, title: title, windowID: 99)) == nil,
          (try? uniqueCaptureWindow(Array(candidates.dropFirst()), pid: 21, title: title, windowID: nil)) == nil else {
        throw GateError("owned terminal window selection failed its auxiliary/PID/visibility/pinned-ID vectors")
    }
    let duplicate = CaptureWindowIdentity(pid: 21, id: 8, title: title, layer: normal, onScreen: true)
    guard (try? uniqueCaptureWindow(candidates + [duplicate], pid: 21, title: title, windowID: nil)) == nil else {
        throw GateError("ambiguous terminal window selection was incorrectly accepted")
    }
}

// The synthetic pattern checks byte ordering and top-down row interpretation.
// It is a prerequisite for interpreting capture pixels, never window evidence.
func rgba(_ image: CGImage) throws -> [UInt8] {
    let width = image.width, height = image.height
    guard width > 0, height > 0, width <= 8192, height <= 8192,
          width * height <= 16 * 1024 * 1024,
          let space = CGColorSpace(name: CGColorSpace.sRGB) else {
        throw GateError("invalid capture dimensions or sRGB color space")
    }
    var bytes = [UInt8](repeating: 0, count: width * height * 4)
    try bytes.withUnsafeMutableBytes { buffer in
        guard let context = CGContext(
            data: buffer.baseAddress, width: width, height: height,
            bitsPerComponent: 8, bytesPerRow: width * 4, space: space,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
                | CGBitmapInfo.byteOrder32Big.rawValue) else {
            throw GateError("cannot allocate the capture normalization context")
        }
        context.interpolationQuality = .none
        context.setBlendMode(.copy)
        context.draw(image, in: CGRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)))
    }
    return bytes
}

func verifyPixelLayout() throws {
    let expected: [UInt8] = [
        255, 0, 0, 255, 0, 255, 0, 255,
        0, 0, 255, 255, 255, 255, 255, 255,
    ]
    guard let provider = CGDataProvider(data: Data(expected) as CFData),
          let space = CGColorSpace(name: CGColorSpace.sRGB),
          let image = CGImage(
            width: 2, height: 2, bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: 8, space: space,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue
                | CGBitmapInfo.byteOrder32Big.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false,
            intent: .defaultIntent),
          try rgba(image) == expected else {
        throw GateError("capture pixel normalization failed the 2x2 orientation/RGBA vector")
    }
}

@MainActor
final class WindowProbe {
    let request: Request
    var report: [String: Any] = [
        "passed": false, "status": "failed",
        "capture": "ScreenCaptureKit desktop-independent WindowServer window",
        "physical_display_latency": "not_measured",
    ]
    var ownedPID: pid_t
    var lastImage: CGImage?
    var imageWindowID: CGWindowID?
    let style: NSWindow.StyleMask = [.titled, .closable, .miniaturizable, .resizable]

    init(_ request: Request) {
        self.request = request
        self.ownedPID = request.pid ?? 0
        self.report["action"] = request.action
    }

    func phase(_ name: String) {
        report["phase"] = name
        FileHandle.standardError.write(Data("ztty-macos-window-probe phase: \(name)\n".utf8))
    }

    func initializeAppKit() throws {
        phase("appkit_initialization")
        guard Thread.isMainThread else {
            throw GateError("AppKit helper initialization requires the main thread")
        }
        // A command-line helper does not get NSApplicationMain automatically.
        // shared establishes the WindowServer connection before NSWindow geometry
        // or screenshot APIs. Prohibited policy prevents helper windows/activation.
        let application = NSApplication.shared
        guard application.setActivationPolicy(.prohibited),
              application.activationPolicy == .prohibited else {
            throw GateError("cannot initialize AppKit with prohibited activation")
        }
        report["appkit"] = [
            "initialized": true, "main_thread": Thread.isMainThread,
            "activation_policy": application.activationPolicy.rawValue,
        ]
    }

    func preflight() throws {
        phase("permission_preflight")
        let capture = CGPreflightScreenCaptureAccess()
        let accessibility = AXIsProcessTrusted()
        report["permissions"] = ["screen_capture": capture, "accessibility": accessibility]
        report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
        #if arch(arm64)
        report["architecture"] = "arm64"
        #elseif arch(x86_64)
        report["architecture"] = "x86_64"
        #else
        report["architecture"] = "unsupported"
        #endif
        var translated: Int32 = 0
        var size = MemoryLayout<Int32>.size
        let result = sysctlbyname("sysctl.proc_translated", &translated, &size, nil, 0)
        report["process_translated"] = result == 0 && translated != 0
        guard result != 0 || translated == 0 else {
            throw GateError("the native helper is running under Rosetta")
        }
        guard capture, accessibility else {
            throw GateError("native window acceptance requires Screen Recording and Accessibility permissions; no permissions were requested or changed", blocked: true)
        }
        try verifyPixelLayout()
        report["pixel_layout_vector"] = "passed"
        try verifyWindowSelection()
        report["window_selection_vector"] = "passed"
    }

    func content() async throws -> SCShareableContent {
        phase("window_inventory")
        return try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: false)
    }

    func ownedWindow(_ content: SCShareableContent) throws -> SCWindow {
        let windows = content.windows.filter { $0.owningApplication?.processID == ownedPID }
        let identities = windows.map {
            CaptureWindowIdentity(pid: ownedPID, id: $0.windowID, title: $0.title,
                                  layer: $0.windowLayer, onScreen: $0.isOnScreen)
        }
        report["window_candidates"] = zip(windows, identities).map { window, identity in
            [
                "pid": ownedPID, "window_id": window.windowID,
                "title": window.title ?? "<unavailable>", "layer": window.windowLayer,
                "on_screen": window.isOnScreen, "frame_points": rectJSON(window.frame),
                "matches_target": identity.matches(
                    pid: ownedPID, title: request.windowTitle ?? "", windowID: request.windowID),
            ] as [String: Any]
        }
        let id = try uniqueCaptureWindow(
            identities, pid: ownedPID, title: request.windowTitle ?? "", windowID: request.windowID)
        guard let selected = windows.first(where: { $0.windowID == id }) else {
            throw GateError("selected terminal WindowID disappeared from the owned inventory")
        }
        return selected
    }

    func accessibilityWindow() throws -> AXUIElement {
        let app = AXUIElementCreateApplication(ownedPID)
        var value: CFTypeRef?
        let error = AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value)
        guard error == .success, let windows = value as? [AXUIElement],
              let expectedTitle = request.windowTitle, !expectedTitle.isEmpty else {
            throw GateError("cannot read the owned AX windows or unique title: \(error.rawValue)")
        }
        func stringAttribute(_ window: AXUIElement, _ name: String) -> String? {
            var value: CFTypeRef?
            guard AXUIElementCopyAttributeValue(window, name as CFString, &value) == .success else {
                return nil
            }
            return value as? String
        }
        var matches: [AXUIElement] = []
        var inventory: [[String: Any]] = []
        for window in windows {
            var pid: pid_t = 0
            let ownerError = AXUIElementGetPid(window, &pid)
            let title = stringAttribute(window, kAXTitleAttribute)
            let role = stringAttribute(window, kAXRoleAttribute)
            let subrole = stringAttribute(window, kAXSubroleAttribute)
            let match = ownerError == .success && pid == ownedPID && title == expectedTitle
                && role == kAXWindowRole && subrole == kAXStandardWindowSubrole
            inventory.append([
                "pid": pid, "pid_error": ownerError.rawValue,
                "title": title ?? "<unavailable>", "role": role ?? "<unavailable>",
                "subrole": subrole ?? "<unavailable>", "matches_target": match,
            ])
            if match { matches.append(window) }
        }
        report["ax_window_candidates"] = inventory
        guard matches.count == 1 else {
            throw GateError("expected one owned standard AX terminal window; matching \(matches.count) of \(windows.count)")
        }
        return matches[0]
    }

    func set(_ window: AXUIElement, _ attribute: CFString, _ value: CFTypeRef) throws {
        var settable: DarwinBoolean = false
        guard AXUIElementIsAttributeSettable(window, attribute, &settable) == .success,
              settable.boolValue else {
            throw GateError("owned window attribute is not settable: \(attribute)")
        }
        let error = AXUIElementSetAttributeValue(window, attribute, value)
        guard error == .success else {
            throw GateError("setting owned window \(attribute) failed: \(error.rawValue)")
        }
    }

    func minimized(_ window: AXUIElement) throws -> Bool {
        var value: CFTypeRef?
        let error = AXUIElementCopyAttributeValue(window, kAXMinimizedAttribute as CFString, &value)
        guard error == .success, let flag = value as? Bool else {
            throw GateError("cannot read owned AX minimized state: \(error.rawValue)")
        }
        return flag
    }

    func raise(_ window: AXUIElement) throws {
        let error = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
        guard error == .success else {
            throw GateError("raising the owned window failed: \(error.rawValue)")
        }
    }

    func capture(_ window: SCWindow, _ content: SCShareableContent) async throws -> Bool {
        phase("capture_geometry")
        let filter = SCContentFilter(desktopIndependentWindow: window)
        let scale = Double(filter.pointPixelScale)
        guard scale.isFinite, scale >= 1, scale <= 8 else {
            throw GateError("invalid WindowServer points-to-pixels scale: \(scale)")
        }
        let frame = window.frame
        let client = NSWindow.contentRect(
            forFrameRect: NSRect(origin: .zero, size: frame.size), styleMask: style)
        let left = Int((client.minX * scale).rounded())
        let top = Int(((frame.height - client.maxY) * scale).rounded())
        let clientWidth = Int((client.width * scale).rounded())
        let clientHeight = Int((client.height * scale).rounded())
        let imageWidth = Int((filter.contentRect.width * scale).rounded())
        let imageHeight = Int((filter.contentRect.height * scale).rounded())
        let visible = content.displays.contains {
            $0.frame.insetBy(dx: -0.5, dy: -0.5).contains(frame)
        }
        report["window"] = [
            "pid": ownedPID, "window_id": window.windowID,
            "title": window.title ?? "<unavailable>", "layer": window.windowLayer,
            "frame_points": rectJSON(frame), "client_points": rectJSON(client),
            "filter_content_points": rectJSON(filter.contentRect),
            "point_pixel_scale": scale,
            "client_pixel_rect": [left, top, clientWidth, clientHeight],
            "on_screen": window.isOnScreen, "inside_display": visible,
            "displays": content.displays.map { ["id": $0.displayID, "frame_points": rectJSON($0.frame)] },
        ]
        guard imageWidth > 0, imageHeight > 0, imageWidth <= 8192, imageHeight <= 8192,
              imageWidth * imageHeight <= 16 * 1024 * 1024 else {
            throw GateError("unbounded or empty WindowServer capture")
        }
        let configuration = SCStreamConfiguration()
        configuration.width = imageWidth
        configuration.height = imageHeight
        configuration.showsCursor = false
        configuration.capturesAudio = false
        configuration.ignoreShadowsSingleWindow = true
        configuration.colorSpaceName = CGColorSpace.sRGB
        phase("windowserver_capture")
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter, configuration: configuration)
        phase("pixels_received")
        lastImage = image
        imageWindowID = window.windowID
        var colorName = "unknown"
        if let name = image.colorSpace?.name { colorName = name as String }
        report["image"] = [
            "width": image.width, "height": image.height,
            "color_space": colorName,
        ]
        guard window.isOnScreen, visible else {
            throw GateError("the owned window is not wholly on an active display")
        }
        guard image.width == imageWidth, image.height == imageHeight,
              left >= 0, top >= 0, clientWidth > 0, clientHeight > 0,
              left + clientWidth <= image.width, top + clientHeight <= image.height else {
            throw GateError("WindowServer image and AppKit client geometry disagree")
        }
        if let expected = request.expectedClientPixels,
           expected != [clientWidth, clientHeight] {
            throw GateError("client pixels \([clientWidth, clientHeight]) do not match PTY pixels \(expected)")
        }
        phase("pixel_comparison")
        let bytes = try rgba(image)
        var matches = true
        var samples: [[String: Any]] = []
        for sample in request.samples ?? [] {
            guard sample.rgb.count == 3, sample.rgb.allSatisfy({ (0...255).contains($0) }),
                  sample.x >= 0, sample.y >= 0, sample.x < clientWidth, sample.y < clientHeight else {
                throw GateError("sample is outside the actual client rectangle")
            }
            let offset = ((top + sample.y) * image.width + left + sample.x) * 4
            let actual = Array(bytes[offset..<(offset + 3)]).map(Int.init)
            let match = zip(actual, sample.rgb).allSatisfy { abs($0.0 - $0.1) <= 1 }
            matches = matches && match
            samples.append(["client": [sample.x, sample.y], "expected": sample.rgb,
                            "actual": actual, "alpha": Int(bytes[offset + 3]), "passed": match])
        }
        report["samples"] = samples
        return matches
    }

    func waitForCapture() async throws {
        let timeout = request.timeout ?? 10
        guard timeout > 0, timeout <= 10 else { throw GateError("invalid capture timeout") }
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        var attempts = 0
        while ProcessInfo.processInfo.systemUptime < deadline {
            attempts += 1
            do {
                if request.action == "restore" {
                    let window = try accessibilityWindow()
                    let isMinimized = try minimized(window)
                    report["minimized"] = isMinimized
                    guard !isMinimized else {
                        throw GateError("owned window is still completing its restore")
                    }
                    try raise(window)
                }
                let available = try await content()
                let window = try ownedWindow(available)
                if try await capture(window, available) {
                    report["attempts"] = attempts
                    return
                }
                report["last_error"] = "actual WindowServer pixels do not match the expected vector"
            } catch {
                report["last_error"] = String(describing: error)
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        report["attempts"] = attempts
        throw GateError("native window capture timed out after \(timeout) seconds")
    }

    func invalidCwd() async throws {
        guard let binary = request.binary, let arguments = request.arguments,
              arguments.contains("--cwd") else {
            throw GateError("invalid-cwd probe requires an explicit binary and --cwd")
        }
        let process = Process()
        let stdout = Pipe(), stderr = Pipe()
        process.executableURL = URL(fileURLWithPath: binary)
        process.arguments = arguments
        process.standardOutput = stdout
        process.standardError = stderr
        let start = ProcessInfo.processInfo.systemUptime
        try process.run()
        ownedPID = process.processIdentifier
        var observed = Set<CGWindowID>()
        var observedFrames: [[String: Any]] = []
        var samples = 0
        func observe() throws {
            guard let list = CGWindowListCopyWindowInfo(
                [.optionAll, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
                throw GateError("cannot enumerate WindowServer windows for invalid-cwd probe")
            }
            samples += 1
            for entry in list where (entry[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == ownedPID {
                if let id = entry[kCGWindowNumber as String] as? NSNumber {
                    if observed.insert(id.uint32Value).inserted {
                        observedFrames.append([
                            "window_id": id.uint32Value,
                            "bounds": entry[kCGWindowBounds as String] ?? NSNull(),
                            "layer": entry[kCGWindowLayer as String] ?? NSNull(),
                        ])
                    }
                }
            }
        }
        var observationError: Error?
        repeat {
            do { try observe() } catch { observationError = error; break }
            if !process.isRunning { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        } while (ProcessInfo.processInfo.systemUptime - start) < 3
        let timedOut = process.isRunning
        // Never leave this owned failing launch alive behind a capture await.
        // Its observed WindowServer IDs/geometry remain evidence if it opened UI.
        if process.isRunning {
            process.terminate()
            let deadline = ProcessInfo.processInfo.systemUptime + 1
            while process.isRunning && ProcessInfo.processInfo.systemUptime < deadline {
                try await Task.sleep(nanoseconds: 20_000_000)
            }
            if process.isRunning { _ = Darwin.kill(ownedPID, SIGKILL) }
        }
        process.waitUntilExit()
        do { try observe() } catch { observationError = error }
        let output = String(decoding: stdout.fileHandleForReading.readDataToEndOfFile().suffix(4096), as: UTF8.self)
        let errors = String(decoding: stderr.fileHandleForReading.readDataToEndOfFile().suffix(4096), as: UTF8.self)
        report["launch"] = [
            "pid": ownedPID, "exit_code": process.terminationStatus,
            "termination_reason": process.terminationReason.rawValue,
            "elapsed_seconds": (ProcessInfo.processInfo.systemUptime - start), "timed_out": timedOut,
            "observed_window_ids": observed.sorted(), "window_observations": samples,
            "observed_window_frames": observedFrames,
            "stdout": output, "stderr": errors,
        ]
        if let error = observationError { throw error }
        guard !timedOut, process.terminationReason == .exit, process.terminationStatus != 0,
              observed.isEmpty, errors.contains("cannot enter command working directory (OS error ") else {
            throw GateError("invalid --cwd did not fail promptly without an observed native window")
        }
    }

    func run() async throws {
        try initializeAppKit()
        try preflight()
        if request.action == "preflight" { return }
        if request.action == "invalid_cwd" { try await invalidCwd(); return }
        guard ownedPID > 0 else { throw GateError("missing owned process ID") }
        guard let title = request.windowTitle, !title.isEmpty else {
            throw GateError("missing unique terminal window title")
        }
        report["target"] = ["pid": ownedPID, "window_id": request.windowID ?? 0, "title": title]
        if request.action == "resize" || request.action == "minimize" || request.action == "restore" {
            let window = try accessibilityWindow()
            if request.action == "resize" {
                let available = try await content()
                let native = try ownedWindow(available)
                let scale = Double(SCContentFilter(desktopIndependentWindow: native).pointPixelScale)
                guard let pixels = request.targetClientPixels, pixels.count == 2,
                      pixels.allSatisfy({ $0 > 0 && $0 <= 8192 }), scale >= 1, scale <= 8 else {
                    throw GateError("invalid target client geometry")
                }
                let frame = NSWindow.frameRect(
                    forContentRect: NSRect(x: 0, y: 0, width: Double(pixels[0]) / scale,
                                           height: Double(pixels[1]) / scale), styleMask: style)
                var size = frame.size
                guard let value = AXValueCreate(.cgSize, &size) else {
                    throw GateError("cannot encode owned AX resize geometry")
                }
                try set(window, kAXSizeAttribute as CFString, value)
                try raise(window)
            } else {
                if request.action == "minimize" {
                    let available = try await content()
                    let native = try ownedWindow(available)
                    report["window_before_action"] = [
                        "pid": ownedPID, "window_id": native.windowID,
                        "frame_points": rectJSON(native.frame), "on_screen": native.isOnScreen,
                    ]
                }
                try set(window, kAXMinimizedAttribute as CFString,
                        request.action == "minimize" ? kCFBooleanTrue! : kCFBooleanFalse!)
                if request.action == "minimize" {
                    let deadline = ProcessInfo.processInfo.systemUptime + (request.timeout ?? 10)
                    while ProcessInfo.processInfo.systemUptime < deadline {
                        let available = try await content()
                        let owned = available.windows.filter {
                            $0.owningApplication?.processID == ownedPID
                                && (request.windowID == nil || $0.windowID == request.windowID)
                        }
                        let isMinimized = try minimized(window)
                        report["minimized"] = isMinimized
                        report["on_screen_window_ids"] = owned.filter(\.isOnScreen).map(\.windowID)
                        if isMinimized && !owned.contains(where: \.isOnScreen) { return }
                        try await Task.sleep(nanoseconds: 20_000_000)
                    }
                    throw GateError("owned window did not become minimized and off-screen")
                }
            }
        } else if request.action != "capture" {
            throw GateError("unknown native probe action: \(request.action)")
        }
        try await waitForCapture()
    }

    func saveEvidence() throws {
        guard let image = lastImage, let path = request.png else { return }
        phase("saving_png")
        let url = URL(fileURLWithPath: path)
        guard let destination = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else {
            throw GateError("cannot create the actual window PNG")
        }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else {
            throw GateError("cannot save the actual window PNG")
        }
        report["screenshot"] = [
            "path": path, "pid": ownedPID, "window_id": imageWindowID ?? 0,
            "width": image.width, "height": image.height,
        ]
    }
}

@main
struct Main {
    @MainActor
    static func main() async {
        var output: [String: Any]
        do {
            guard CommandLine.arguments.count == 2 else {
                throw GateError("usage: macos_window_probe request.json")
            }
            let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
            let request = try JSONDecoder().decode(Request.self, from: data)
            let probe = WindowProbe(request)
            do {
                try await probe.run()
                probe.report["passed"] = true
                probe.report["status"] = "passed"
            } catch {
                probe.report["error"] = String(describing: error)
                probe.report["status"] = (error as? GateError)?.blocked == true ? "blocked" : "failed"
            }
            do { try probe.saveEvidence() } catch {
                probe.report["passed"] = false
                probe.report["status"] = "failed"
                probe.report["screenshot_error"] = String(describing: error)
            }
            output = probe.report
        } catch {
            output = ["passed": false, "status": "failed", "error": String(describing: error)]
        }
        do {
            let data = try JSONSerialization.data(withJSONObject: output, options: [.prettyPrinted, .sortedKeys])
            FileHandle.standardOutput.write(data)
            FileHandle.standardOutput.write(Data([10]))
        } catch {
            FileHandle.standardError.write(Data("cannot encode native helper report: \(error)\n".utf8))
            exit(1)
        }
        exit(output["passed"] as? Bool == true ? 0 : (output["status"] as? String == "blocked" ? 2 : 1))
    }
}
