// Render the canonical Web PP mark into iOS icon canvases; no redrawn logo.
// swift scripts/app-icons.swift <AppIcon.appiconset> [mark.png]
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers
let output = URL(fileURLWithPath: CommandLine.arguments[1])
let sourceURL = URL(fileURLWithPath: CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "assets/brand/mark.png")
guard let source = CGImageSourceCreateWithURL(sourceURL as CFURL, nil),
      let mark = CGImageSourceCreateImageAtIndex(source, 0, nil) else { fatalError("Missing canonical brand mark") }
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
for (name, dark) in [("AppIcon.png", false), ("AppIcon-dark.png", true), ("AppIcon-tinted.png", true)] {
  let context = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
  context.setFillColor(CGColor(gray: dark ? 0.06 : 1, alpha: 1))
  context.fill(CGRect(x: 0, y: 0, width: 1024, height: 1024))
  context.interpolationQuality = .high
  let rect = CGRect(x: 128, y: 128, width: 768, height: 768)
  if name.contains("tinted") {
    context.saveGState()
    context.clip(to: rect, mask: mark)
    context.setFillColor(CGColor(gray: 1, alpha: 1))
    context.fill(rect)
    context.restoreGState()
  } else { context.draw(mark, in: rect) }
  let destination = CGImageDestinationCreateWithURL(output.appendingPathComponent(name) as CFURL, UTType.png.identifier as CFString, 1, nil)!
  CGImageDestinationAddImage(destination, context.makeImage()!, nil)
  guard CGImageDestinationFinalize(destination) else { fatalError("Cannot write icon") }
}
