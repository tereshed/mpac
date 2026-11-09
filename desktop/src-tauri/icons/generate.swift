// Reproducible native icon rendering. No window or application is opened.
// Run: swift -module-cache-path .mpac/icon-module-cache \
//   desktop/src-tauri/icons/generate.swift desktop/src-tauri/icons .mpac/MPAC.iconset
import AppKit
import Foundation

let arguments = CommandLine.arguments
if arguments.count != 3 {
    fputs("Usage: generate.swift <icons-directory> <iconset-directory>\n", stderr)
    exit(2)
}
let output = URL(fileURLWithPath: arguments[1], isDirectory: true)
let iconset = URL(fileURLWithPath: arguments[2], isDirectory: true)
let manager = FileManager.default
try manager.createDirectory(at: output, withIntermediateDirectories: true)
try manager.createDirectory(at: iconset, withIntermediateDirectories: true)

func color(_ red: CGFloat, _ green: CGFloat, _ blue: CGFloat) -> NSColor {
    NSColor(srgbRed: red / 255, green: green / 255, blue: blue / 255, alpha: 1)
}
func render(_ size: Int) throws -> Data {
    guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: size * 4, bitsPerPixel: 32), let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
        throw NSError(domain: "MPACIcon", code: 1, userInfo: [NSLocalizedDescriptionKey: "Unable to create icon bitmap"])
    }
    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    NSGraphicsContext.current = context
    context.shouldAntialias = true
    context.cgContext.clear(CGRect(x: 0, y: 0, width: size, height: size))
    let scale = CGFloat(size) / 1024
    context.cgContext.scaleBy(x: scale, y: scale)
    let box = NSBezierPath(roundedRect: NSRect(x: 64, y: 64, width: 896, height: 896), xRadius: 200, yRadius: 200)
    color(34, 51, 42).setFill()
    box.fill()
    color(71, 96, 75).setStroke()
    box.lineWidth = 12
    box.stroke()
    let attributes: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: 600, weight: .semibold),
        .foregroundColor: color(201, 242, 195)
    ]
    // The NSString line origin includes the system font descent. Keep the ink
    // centered alongside the dot, matching the source SVG mark.
    ("m" as NSString).draw(at: NSPoint(x: 167, y: 205), withAttributes: attributes)
    color(178, 222, 131).setFill()
    NSBezierPath(ovalIn: NSRect(x: 756, y: 469, width: 86, height: 86)).fill()
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
        throw NSError(domain: "MPACIcon", code: 2, userInfo: [NSLocalizedDescriptionKey: "Unable to encode PNG"])
    }
    return data
}
let names: [(String, Int)] = [
    ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024)
]
for (name, size) in names {
    try render(size).write(to: iconset.appendingPathComponent(name))
}
for (name, size) in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("512x512.png", 512), ("icon.png", 1024)] {
    try render(size).write(to: output.appendingPathComponent(name))
}
print("Rendered MPAC PNG icons and macOS iconset")
