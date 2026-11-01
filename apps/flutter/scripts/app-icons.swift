// The iOS app icons (#535, the canvas's 应用图标 board), drawn here so that
// they can be redrawn: 1024 px, opaque, full bleed (the system rounds them).
// The brand gradient with its top-right highlight under the brand mark's
// mountain; the dark and tinted variants of iOS 18.
//
//   swift scripts/app-icons.swift runner/ios/Runner/Assets.xcassets/AppIcon.appiconset
//
// Android's adaptive icon is a vector (runner/android res), nothing to draw.
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let size = 1024
let unit = CGFloat(size) / 24  // the mark's 24-unit grid

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
  CGColor(
    srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
    blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

/// A path in the mark's units, y down.
func path(_ points: [(CGFloat, CGFloat)]) -> CGPath {
  let p = CGMutablePath()
  p.addLines(between: points.map { CGPoint(x: $0.0 * unit, y: $0.1 * unit) })
  p.closeSubpath()
  return p
}

// M3.5 17.5 9.2 9l3.3 4.8 2.1-2.9 5.9 6.6z and M9.2 9l1.4 2.1-1.4.8-1.3-1z
let mountain = path([(3.5, 17.5), (9.2, 9), (12.5, 13.8), (14.6, 10.9), (20.5, 17.5)])
let snow = path([(9.2, 9), (10.6, 11.1), (9.2, 11.9), (7.9, 10.9)])

enum Variant { case light, dark, tinted }

func draw(_ variant: Variant, to url: URL) throws {
  let space = CGColorSpace(name: CGColorSpace.sRGB)!
  let ctx = CGContext(
    data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space,
    bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
  // Y down, as the SVG.
  ctx.translateBy(x: 0, y: CGFloat(size))
  ctx.scaleBy(x: 1, y: -1)
  let all = CGRect(x: 0, y: 0, width: size, height: size)
  switch variant {
  case .light:
    let fill = CGGradient(colorsSpace: space, colors: [color(0x3D63C9), color(0x1F4390)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(fill, start: .zero, end: CGPoint(x: size, y: size), options: [])
    let glow = CGGradient(
      colorsSpace: space, colors: [color(0xFFFFFF, 0.22), color(0xFFFFFF, 0)] as CFArray, locations: [0, 1])!
    let center = CGPoint(x: 0.85 * CGFloat(size), y: 0.1 * CGFloat(size))
    ctx.drawRadialGradient(
      glow, startCenter: center, startRadius: 0, endCenter: center, endRadius: 0.7 * CGFloat(size), options: [])
    ctx.addPath(mountain)
    ctx.setFillColor(color(0xFFFFFF))
    ctx.fillPath()
    ctx.addPath(snow)
    ctx.setFillColor(color(0xC9D6FF))
    ctx.fillPath()
  case .dark:
    ctx.setFillColor(color(0x0D0F13))
    ctx.fill(all)
    ctx.saveGState()
    ctx.addPath(mountain)
    ctx.clip()
    let fill = CGGradient(colorsSpace: space, colors: [color(0xB4C5FF), color(0x7F9CF0)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(fill, start: .zero, end: CGPoint(x: size, y: size), options: [])
    ctx.restoreGState()
    ctx.addPath(snow)
    ctx.setFillColor(color(0xE3E9FF))
    ctx.fillPath()
  case .tinted:
    ctx.setFillColor(color(0x1A1A1A))
    ctx.fill(all)
    ctx.addPath(mountain)
    ctx.setFillColor(color(0xD9D9D9))
    ctx.fillPath()
    ctx.addPath(snow)
    ctx.setFillColor(color(0xFFFFFF))
    ctx.fillPath()
  }
  let image = ctx.makeImage()!
  let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
  CGImageDestinationAddImage(dest, image, nil)
  guard CGImageDestinationFinalize(dest) else { throw CocoaError(.fileWriteUnknown) }
}

let dir = URL(fileURLWithPath: CommandLine.arguments.dropFirst().first ?? ".")
try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
try draw(.light, to: dir.appendingPathComponent("AppIcon.png"))
try draw(.dark, to: dir.appendingPathComponent("AppIcon-dark.png"))
try draw(.tinted, to: dir.appendingPathComponent("AppIcon-tinted.png"))
