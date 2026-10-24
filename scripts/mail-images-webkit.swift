// macOS WebKit regression for the real reader and reply-preview mount functions.
// Run: swift scripts/mail-images-webkit.swift [--baseline]
// --baseline reads HEAD to demonstrate the regression before an uncommitted fix.
// Only synthetic mail is used; the iframe CSP blocks all network requests.
import Cocoa
import WebKit

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let baseline = CommandLine.arguments.contains("--baseline")
func source(_ path: String) throws -> String {
    if !baseline { return try String(contentsOf: root.appendingPathComponent(path), encoding: .utf8) }
    let process = Process(), output = Pipe()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    process.arguments = ["-C", root.path, "show", "HEAD:\(path)"]
    process.standardOutput = output
    try process.run()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else { fatalError("Cannot read baseline \(path)") }
    return String(data: data, encoding: .utf8)!
}
func function(_ name: String, in source: String, optional: Bool = false) -> String {
    guard let start = source.range(of: "function \(name)(") else {
        if optional { return "" }
        fatalError("Missing function \(name)")
    }
    guard let end = source.range(of: "\n}", range: start.upperBound..<source.endIndex) else {
        fatalError("Unterminated function \(name)")
    }
    return String(source[start.lowerBound..<end.upperBound])
}
func json(_ value: Any) -> String {
    String(data: try! JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed]), encoding: .utf8)!
        .replacingOccurrences(of: "<", with: "\\u003c")
}
let appSource = try source("ui/app.js")
let quoteSource = try source("ui/compose-quote.js")
let functions = ["fitMailFrame", "mountFrame", "mountComposeQuote"].map { function($0, in: appSource) }.joined(separator: "\n")
    + "\n" + function("sizeMailImages", in: appSource, optional: true)
let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='1600' height='400'><rect width='1600' height='400' fill='orange'/></svg>"
let good = "data:image/svg+xml;base64," + Data(svg.utf8).base64EncodedString()
let bad = "data:image/png;base64,bm90LWFuLWltYWdl"
let body = """
<table width="920"><tr><td><img id="banner" src="https://example.invalid/banner.jpg" width="100%" height="83" alt="Banner"></td></tr>
<tr><td><p id="body-start">Statement body remains visible</p></td></tr>
<tr><td><img id="details" src="\(bad)" width="875" height="26" alt="Details"></td></tr>
<tr><td><img id="large" src="\(good)" width="1600" height="400" alt="Large loaded image"></td></tr>
<tr><td><img id="inline" src="\(good)" width="192" height="48" style="height:33px" alt="Inline sizing"></td></tr>
<tr><td><img id="plain" src="\(good)" alt="No dimensions"></td></tr></table>
"""
let application = NSApplication.shared
application.setActivationPolicy(.prohibited)
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 1050, height: 1000))
let window = NSWindow(contentRect: web.frame, styleMask: [.borderless], backing: .buffered, defer: false)
window.contentView = web
var failures = 0, checks = 0, kindIndex = 0
let kinds = ["reader", "quote"]
func check(_ name: String, _ okay: Bool, _ detail: String = "") {
    checks += 1
    if !okay { failures += 1 }
    print("\(okay ? "PASS" : "FAIL") \(name)\(detail.isEmpty ? "" : " — " + detail)")
    fflush(stdout)
}
let geometry = """
(()=>{const d=document.querySelector('iframe').contentDocument;
const item=id=>{const i=d.getElementById(id),r=i.getBoundingClientRect();return {width:r.width,height:r.height,loaded:i.hasAttribute('data-mailbeacon-loaded'),naturalWidth:i.naturalWidth};};
return {banner:item('banner'),details:item('details'),large:item('large'),inline:item('inline'),plain:item('plain'),bodyTop:d.getElementById('body-start').getBoundingClientRect().top};})()
"""
func evaluate(_ code: String, _ completion: @escaping (Any?) -> Void) {
    web.evaluateJavaScript(code) { value, error in
        if let error = error { check("WebKit evaluation", false, error.localizedDescription) }
        completion(value)
    }
}
func afterLayout(_ completion: @escaping ([String: Any]) -> Void) {
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
        evaluate(geometry) { value in completion(value as? [String: Any] ?? [:]) }
    }
}
func image(_ result: [String: Any], _ id: String) -> [String: Any] { result[id] as? [String: Any] ?? [:] }
func number(_ result: [String: Any], _ field: String) -> Double { (result[field] as? NSNumber)?.doubleValue ?? -1 }
func close(_ value: Double, _ expected: Double) -> Bool { abs(value - expected) < 1 }
func runKind() {
    if kindIndex == kinds.count {
        print("\(checks - failures)/\(checks) WebKit image checks passed\(baseline ? " (HEAD baseline)" : "")")
        exit(failures == 0 ? 0 : 1)
    }
    let kind = kinds[kindIndex]
    let id = kind == "reader" ? "mail-frame" : "compose-quote-frame"
    let script = """
    window.__ready=false;
    const $=(selector,scope=document)=>scope.querySelector(selector),key=(a,b)=>a+':'+b;
    const S={showImages:new Set()},onKey=()=>{},openMailImage=()=>{},watchReaderScrollInput=()=>{};
    const keepMailFragment=()=>false,invoke=async()=>{},fail=()=>{};
    \(quoteSource)
    \(functions)
    const f=document.querySelector('iframe');f.addEventListener('load',()=>{window.__ready=true;});
    \(kind == "reader"
      ? "mountFrame({html:" + json(body) + ",from:{addr:'sender@example.invalid'}},{accountId:'a',folder:'INBOX',uid:1});"
      : "const sheet=document.body;sheet._quotePreview={html:" + json(body) + "};mountComposeQuote(sheet);")
    """
    let outer = "<!doctype html><style>body{margin:0}iframe{display:block;width:100%;border:0;height:200px}</style><iframe id='\(id)' sandbox='allow-same-origin allow-popups' referrerpolicy='no-referrer'></iframe><script>eval(\(json(script)))</script>"
    web.loadHTMLString(outer, baseURL: nil)
    func waitForFrame(_ attempts: Int) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
            evaluate("window.__ready === true") { ready in
                if (ready as? Bool) == true {
                    afterLayout { result in
                        let banner = image(result, "banner"), details = image(result, "details")
                        let large = image(result, "large"), inline = image(result, "inline"), plain = image(result, "plain")
                        check("\(kind) blocked percentage banner keeps 83px height", close(number(banner,"height"),83), json(banner))
                        check("\(kind) failed numeric banner keeps 26px height", close(number(details,"height"),26), json(details))
                        check("\(kind) statement begins below compact banner", number(result,"bodyTop") > 0 && number(result,"bodyTop") < 180, "top=\(number(result,"bodyTop"))")
                        check("\(kind) loaded large image scales proportionally", number(large,"width") < 1600 && close(number(large,"height"),number(large,"width")/4), json(large))
                        check("\(kind) authored inline image height wins", close(number(inline,"height"),33), json(inline))
                        check("\(kind) image without dimensions scales proportionally", number(plain,"width") > 0 && close(number(plain,"height"),number(plain,"width")/4), json(plain))
                        evaluate("document.querySelector('iframe').contentDocument.getElementById('banner').src=" + json(good)) { _ in
                            afterLayout { result in
                                let banner = image(result,"banner")
                                check("\(kind) later image load enables proportional height", banner["loaded"] as? Bool == true && close(number(banner,"height"),number(banner,"width")/4), json(banner))
                                evaluate("document.querySelector('iframe').contentDocument.getElementById('banner').src=" + json(bad)) { _ in
                                    afterLayout { result in
                                        let banner = image(result,"banner")
                                        check("\(kind) later image error restores declared height", banner["loaded"] as? Bool == false && close(number(banner,"height"),83), json(banner))
                                        kindIndex += 1; runKind()
                                    }
                                }
                            }
                        }
                    }
                } else if attempts > 0 { waitForFrame(attempts - 1) }
                else { check("\(kind) iframe mounted",false);kindIndex += 1;runKind() }
            }
        }
    }
    waitForFrame(50)
}
runKind()
application.run()
