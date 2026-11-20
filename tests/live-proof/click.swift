import CoreGraphics
import Foundation
// click.swift <x> <y> [right]: one real mouse click (a status item opens its menu only for a real click).
let a = CommandLine.arguments
let p = CGPoint(x: Double(a[1])!, y: Double(a[2])!)
let right = a.count > 3 && a[3] == "right"
let down = CGEvent(mouseEventSource: nil, mouseType: right ? .rightMouseDown : .leftMouseDown, mouseCursorPosition: p, mouseButton: right ? .right : .left)!
down.post(tap: .cghidEventTap)
usleep(120000)
let up = CGEvent(mouseEventSource: nil, mouseType: right ? .rightMouseUp : .leftMouseUp, mouseCursorPosition: p, mouseButton: right ? .right : .left)!
up.post(tap: .cghidEventTap)
