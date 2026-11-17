// Draws the Memory Watchdog app icon: a glowing memory chip with a heartbeat
// line on a dark squircle. Usage: swift make-icon.swift <out.png>  (1024x1024)
import AppKit
import CoreGraphics

let S: CGFloat = 1024
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon-1024.png"
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: Int(S), height: Int(S), bitsPerComponent: 8, bytesPerRow: 0,
                    space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
// Flip so y grows downward, like a design canvas.
ctx.translateBy(x: 0, y: S); ctx.scaleBy(x: 1, y: -1)

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255, alpha: a)
}
func grad(_ colors: [CGColor], _ locs: [CGFloat]? = nil) -> CGGradient {
    CGGradient(colorsSpace: cs, colors: colors as CFArray, locations: locs)!
}
func rounded(_ r: CGRect, _ radius: CGFloat) -> CGPath {
    CGPath(roundedRect: r, cornerWidth: radius, cornerHeight: radius, transform: nil)
}

// macOS icon grid: 824pt body inside the 1024 canvas, with a drop shadow.
let body = CGRect(x: 100, y: 100, width: 824, height: 824)
let squircle = rounded(body, 185)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: 12), blur: 28, color: rgb(0x000000, 0.45))
ctx.addPath(squircle); ctx.setFillColor(rgb(0x10122a)); ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(squircle); ctx.clip()
ctx.drawLinearGradient(grad([rgb(0x2a1b54), rgb(0x0d0f24)]),
                       start: CGPoint(x: 512, y: 100), end: CGPoint(x: 512, y: 924), options: [])
// Warm glow behind the chip.
ctx.drawRadialGradient(grad([rgb(0xff4d6d, 0.55), rgb(0xff4d6d, 0)]),
                       startCenter: CGPoint(x: 512, y: 512), startRadius: 0,
                       endCenter: CGPoint(x: 512, y: 512), endRadius: 400, options: [])
ctx.restoreGState()

// Chip pins: four per side.
let chip = CGRect(x: 262, y: 262, width: 500, height: 500)
let pinLen: CGFloat = 70, pinW: CGFloat = 46
ctx.setFillColor(rgb(0xffc56b))
for i in 0..<4 {
    let t = chip.minY + 78 + CGFloat(i) * 108
    for r in [CGRect(x: chip.minX - pinLen + 10, y: t, width: pinLen, height: pinW),
              CGRect(x: chip.maxX - 10, y: t, width: pinLen, height: pinW),
              CGRect(x: t, y: chip.minY - pinLen + 10, width: pinW, height: pinLen),
              CGRect(x: t, y: chip.maxY - 10, width: pinW, height: pinLen)] {
        ctx.addPath(rounded(r, 14)); ctx.fillPath()
    }
}

// Chip body with an amber-to-pink gradient.
let chipPath = rounded(chip, 70)
ctx.saveGState()
ctx.setShadow(offset: .zero, blur: 60, color: rgb(0xff5577, 0.8))
ctx.addPath(chipPath); ctx.setFillColor(rgb(0xff7a59)); ctx.fillPath()
ctx.restoreGState()
ctx.saveGState()
ctx.addPath(chipPath); ctx.clip()
ctx.drawLinearGradient(grad([rgb(0xffb547), rgb(0xff4f7b)]),
                       start: CGPoint(x: chip.minX, y: chip.minY), end: CGPoint(x: chip.maxX, y: chip.maxY), options: [])
// Inner die, darker, where the pulse line sits.
let die = chip.insetBy(dx: 60, dy: 60)
ctx.addPath(rounded(die, 40)); ctx.setFillColor(rgb(0x1a0f2e, 0.92)); ctx.fillPath()
ctx.restoreGState()

// Heartbeat line across the die.
let y0 = die.midY + 10
let pts: [CGPoint] = [
    CGPoint(x: die.minX + 28, y: y0),
    CGPoint(x: die.minX + 105, y: y0),
    CGPoint(x: die.minX + 145, y: y0 - 70),
    CGPoint(x: die.minX + 190, y: y0 + 105),
    CGPoint(x: die.minX + 240, y: y0 - 150),
    CGPoint(x: die.minX + 280, y: y0),
    CGPoint(x: die.maxX - 28, y: y0),
]
let pulse = CGMutablePath(); pulse.addLines(between: pts)
ctx.saveGState()
ctx.setLineCap(.round); ctx.setLineJoin(.round)
ctx.setShadow(offset: .zero, blur: 30, color: rgb(0x5cffc8, 0.9))
ctx.addPath(pulse); ctx.setLineWidth(40); ctx.setStrokeColor(rgb(0x5cffc8)); ctx.strokePath()
ctx.restoreGState()
ctx.setLineCap(.round); ctx.setLineJoin(.round)
ctx.addPath(pulse); ctx.setLineWidth(16); ctx.setStrokeColor(rgb(0xeafff7)); ctx.strokePath()

// Subtle top highlight on the squircle.
ctx.saveGState()
ctx.addPath(squircle); ctx.clip()
ctx.drawLinearGradient(grad([rgb(0xffffff, 0.10), rgb(0xffffff, 0)]),
                       start: CGPoint(x: 512, y: 100), end: CGPoint(x: 512, y: 420), options: [])
ctx.restoreGState()

let img = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: img)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
