// Renders the prdesk app icon (a git pull-request / branch glyph on a dark
// rounded tile) to a PNG at an arbitrary size. Pure CoreGraphics so it stays
// crisp at every icon size.
//
//   swift render_icon.swift <size> <out.png>

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
let size = args.count > 1 ? Int(args[1]) ?? 1024 : 1024
let outPath = args.count > 2 ? args[2] : "icon.png"
let S = CGFloat(size)
let k = S / 1024.0

let cs = CGColorSpaceCreateDeviceRGB()
guard
    let ctx = CGContext(
        data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
        space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else { fatalError("no context") }

// Work in a top-left coordinate space.
ctx.translateBy(x: 0, y: S)
ctx.scaleBy(x: 1, y: -1)

func P(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x * k, y: y * k) }
func L(_ v: CGFloat) -> CGFloat { v * k }
func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255, alpha: a)
}

// --- Tile -------------------------------------------------------------------
let inset: CGFloat = 96
let body = CGRect(x: L(inset), y: L(inset), width: L(1024 - 2 * inset), height: L(1024 - 2 * inset))
let radius = L(185)
let tile = CGPath(roundedRect: body, cornerWidth: radius, cornerHeight: radius, transform: nil)

ctx.saveGState()
ctx.addPath(tile)
ctx.clip()
let grad = CGGradient(
    colorsSpace: cs, colors: [rgb(0x323a49), rgb(0x1f232b)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(
    grad, start: CGPoint(x: 0, y: L(inset)), end: CGPoint(x: 0, y: L(1024 - inset)), options: [])
ctx.restoreGState()

// Hairline edge for crispness.
ctx.saveGState()
ctx.addPath(tile)
ctx.setStrokeColor(rgb(0xffffff, 0.06))
ctx.setLineWidth(L(3))
ctx.strokePath()
ctx.restoreGState()

// --- Glyph: git pull request ------------------------------------------------
let blue = rgb(0x7fb2f0)
let green = rgb(0xa1c181)
let strokeW = L(64)
let dotR = L(60)

ctx.setLineCap(.round)
ctx.setLineJoin(.round)

func dot(_ c: CGPoint, _ color: CGColor) {
    ctx.setFillColor(color)
    ctx.fillEllipse(in: CGRect(x: c.x - dotR, y: c.y - dotR, width: dotR * 2, height: dotR * 2))
}

// Left branch — "main": dot — line — dot.
let aTop = P(392, 348)
let aBot = P(392, 676)
ctx.setStrokeColor(blue)
ctx.setLineWidth(strokeW)
ctx.beginPath()
ctx.move(to: aTop)
ctx.addLine(to: aBot)
ctx.strokePath()

// Right branch — the proposed change: a head commit at top-right whose line
// sweeps down and merges back into main near the bottom (a pull request).
let cTip = P(636, 348)
ctx.setStrokeColor(green)
ctx.setLineWidth(strokeW)
ctx.beginPath()
ctx.move(to: P(636, 412))
ctx.addLine(to: P(636, 470))
ctx.addCurve(to: P(392, 600), control1: P(636, 556), control2: P(548, 600))
ctx.strokePath()

// Nodes.
dot(aTop, blue)
dot(aBot, blue)
dot(cTip, green)

// --- Write PNG --------------------------------------------------------------
guard let image = ctx.makeImage() else { fatalError("no image") }
let url = URL(fileURLWithPath: outPath)
guard
    let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
else { fatalError("no dest") }
CGImageDestinationAddImage(dest, image, nil)
if !CGImageDestinationFinalize(dest) { fatalError("write failed") }
print("wrote \(outPath) @ \(size)px")
