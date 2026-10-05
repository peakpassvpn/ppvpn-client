#!/usr/bin/env swift

import AppKit
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

guard CommandLine.arguments.count == 3 else {
    fputs("usage: generate-app-icon.swift <brand-image.png> <app-icon.png>\n", stderr)
    exit(2)
}

let sourceURL = URL(fileURLWithPath: CommandLine.arguments[1])
let outputURL = URL(fileURLWithPath: CommandLine.arguments[2])

guard
    let source = CGImageSourceCreateWithURL(sourceURL as CFURL, nil),
    let brandImage = CGImageSourceCreateImageAtIndex(source, 0, nil)
else {
    fputs("unable to read brand image: \(sourceURL.path)\n", stderr)
    exit(1)
}

let canvasSize = 1024
let colorSpace = CGColorSpaceCreateDeviceRGB()
guard let context = CGContext(
    data: nil,
    width: canvasSize,
    height: canvasSize,
    bitsPerComponent: 8,
    bytesPerRow: 0,
    space: colorSpace,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
) else {
    fputs("unable to create bitmap context\n", stderr)
    exit(1)
}

context.clear(CGRect(x: 0, y: 0, width: canvasSize, height: canvasSize))
context.interpolationQuality = .high

// macOS-style icon plate: visible transparent corners, generous system-safe inset,
// and a subtle outline that keeps the white tile legible on light backgrounds.
// Apple system apps such as Music and Notes use a measured non-transparent
// footprint of 216/256 (84.375%). Match that footprint exactly at 1024 px.
let plate = CGRect(x: 80, y: 80, width: 864, height: 864)
let platePath = CGPath(
    roundedRect: plate,
    cornerWidth: 194,
    cornerHeight: 194,
    transform: nil
)
context.addPath(platePath)
context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
context.fillPath()

context.saveGState()
context.addPath(platePath)
context.clip()

// Crop the oversized square favicon around the actual brand mark, then place it
// inside the plate without touching the rounded-square safe area.
let sourceCrop = CGRect(x: 64, y: 64, width: 1200, height: 1200)
if let croppedBrand = brandImage.cropping(to: sourceCrop) {
    context.draw(croppedBrand, in: plate)
} else {
    context.draw(brandImage, in: plate)
}
context.restoreGState()

context.addPath(platePath)
context.setStrokeColor(CGColor(red: 0.84, green: 0.88, blue: 0.95, alpha: 1))
context.setLineWidth(2)
context.strokePath()

guard let outputImage = context.makeImage() else {
    fputs("unable to render app icon\n", stderr)
    exit(1)
}

guard
    let destination = CGImageDestinationCreateWithURL(
        outputURL as CFURL,
        UTType.png.identifier as CFString,
        1,
        nil
    )
else {
    fputs("unable to create output: \(outputURL.path)\n", stderr)
    exit(1)
}

CGImageDestinationAddImage(destination, outputImage, nil)
guard CGImageDestinationFinalize(destination) else {
    fputs("unable to write output: \(outputURL.path)\n", stderr)
    exit(1)
}
