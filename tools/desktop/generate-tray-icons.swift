#!/usr/bin/env swift

import AppKit
import Foundation

guard CommandLine.arguments.count == 3 else {
    fputs("usage: generate-tray-icons.swift <source-png> <output-directory>\n", stderr)
    exit(2)
}

let sourceURL = URL(fileURLWithPath: CommandLine.arguments[1])
let outputURL = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)

guard let source = NSImage(contentsOf: sourceURL) else {
    fputs("unable to load source image: \(sourceURL.path)\n", stderr)
    exit(3)
}

try FileManager.default.createDirectory(at: outputURL, withIntermediateDirectories: true)

let states: [(name: String, color: NSColor)] = [
    ("grey", NSColor(srgbRed: 0.56, green: 0.56, blue: 0.58, alpha: 1)),
    ("blue", NSColor(srgbRed: 0.04, green: 0.52, blue: 1.00, alpha: 1)),
    ("green", NSColor(srgbRed: 0.19, green: 0.82, blue: 0.35, alpha: 1)),
    ("red", NSColor(srgbRed: 1.00, green: 0.27, blue: 0.23, alpha: 1)),
]

func render(size: Int, state: (name: String, color: NSColor), suffix: String) throws {
    guard let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: size,
        pixelsHigh: size,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    ), let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
        throw NSError(domain: "PPVPNIconGenerator", code: 1)
    }

    bitmap.size = NSSize(width: size, height: size)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    context.imageInterpolation = .high
    context.cgContext.clear(CGRect(x: 0, y: 0, width: size, height: size))

    source.draw(
        in: NSRect(x: 0, y: 0, width: size, height: size),
        from: NSRect(origin: .zero, size: source.size),
        operation: .sourceOver,
        fraction: 1,
        respectFlipped: true,
        hints: [.interpolation: NSImageInterpolation.high]
    )

    let unit = CGFloat(size)
    let center = NSPoint(x: unit * 0.79, y: unit * 0.21)
    let outerRadius = unit * 0.14
    let innerRadius = unit * 0.098
    let outerRect = NSRect(
        x: center.x - outerRadius,
        y: center.y - outerRadius,
        width: outerRadius * 2,
        height: outerRadius * 2
    )
    NSColor.white.withAlphaComponent(0.96).setFill()
    NSBezierPath(ovalIn: outerRect).fill()

    let innerRect = NSRect(
        x: center.x - innerRadius,
        y: center.y - innerRadius,
        width: innerRadius * 2,
        height: innerRadius * 2
    )
    state.color.setFill()
    NSBezierPath(ovalIn: innerRect).fill()

    context.flushGraphics()
    NSGraphicsContext.restoreGraphicsState()

    guard let data = bitmap.representation(using: .png, properties: [:]) else {
        throw NSError(domain: "PPVPNIconGenerator", code: 2)
    }
    let destination = outputURL.appendingPathComponent("tray-\(state.name)\(suffix).png")
    try data.write(to: destination, options: .atomic)
}

for state in states {
    try render(size: 32, state: state, suffix: "")
    try render(size: 64, state: state, suffix: "@2x")
}
