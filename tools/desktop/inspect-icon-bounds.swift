#!/usr/bin/env swift

import AppKit
import Foundation

for path in CommandLine.arguments.dropFirst() {
    guard
        let image = NSImage(contentsOfFile: path),
        let tiff = image.tiffRepresentation,
        let bitmap = NSBitmapImageRep(data: tiff)
    else {
        print("\(path): unreadable")
        continue
    }

    var minX = bitmap.pixelsWide
    var minY = bitmap.pixelsHigh
    var maxX = -1
    var maxY = -1

    for y in 0..<bitmap.pixelsHigh {
        for x in 0..<bitmap.pixelsWide {
            if (bitmap.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0.01 {
                minX = min(minX, x)
                minY = min(minY, y)
                maxX = max(maxX, x)
                maxY = max(maxY, y)
            }
        }
    }

    if maxX >= minX, maxY >= minY {
        let width = maxX - minX + 1
        let height = maxY - minY + 1
        let widthRatio = Double(width) / Double(bitmap.pixelsWide)
        let heightRatio = Double(height) / Double(bitmap.pixelsHigh)
        print(
            "\(path): canvas=\(bitmap.pixelsWide)x\(bitmap.pixelsHigh) " +
            "bounds=\(width)x\(height) ratio=\(String(format: "%.3f", widthRatio))x" +
            "\(String(format: "%.3f", heightRatio))"
        )
    } else {
        print("\(path): fully transparent")
    }
}
