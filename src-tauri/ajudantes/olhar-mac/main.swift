// dn.os · fase 2C-1. Leitura pontual: não instala event tap, não ativa apps,
// não envia entrada e não pede permissões. Pode ser validada antes da integração.
import Cocoa
import ApplicationServices

func atributo(_ el: AXUIElement, _ nome: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(el, nome as CFString, &v) == .success ? v : nil
}
func texto(_ el: AXUIElement, _ nome: String) -> String {
    guard let v = atributo(el, nome) else { return "" }
    let s = (v as? String) ?? (v as? NSNumber)?.stringValue ?? ""
    return String(s.unicodeScalars.filter { !CharacterSet.controlCharacters.contains($0) }.prefix(240))
}
func elemento(_ v: AnyObject?) -> AXUIElement? {
    guard let v = v, CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
    return (v as! AXUIElement)
}
func retangulo(_ el: AXUIElement) -> CGRect? {
    guard let p = atributo(el, kAXPositionAttribute), let s = atributo(el, kAXSizeAttribute),
          CFGetTypeID(p) == AXValueGetTypeID(), CFGetTypeID(s) == AXValueGetTypeID() else { return nil }
    var pt = CGPoint.zero, sz = CGSize.zero
    guard AXValueGetValue(p as! AXValue, .cgPoint, &pt), AXValueGetValue(s as! AXValue, .cgSize, &sz) else { return nil }
    return CGRect(origin: pt, size: sz)
}
func retJSON(_ r: CGRect) -> [String: Double] {
    return ["x": Double(r.minX), "y": Double(r.minY), "w": Double(r.width), "h": Double(r.height)]
}
func telaPermitida() -> Bool {
    typealias Fn = @convention(c) () -> Bool
    guard let h = dlopen("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics", RTLD_LAZY) else { return false }
    defer { dlclose(h) }
    guard let f = dlsym(h, "CGPreflightScreenCaptureAccess") else { return false }
    return unsafeBitCast(f, to: Fn.self)()
}
func resposta(_ valor: [String: Any]) -> Never {
    let data = try! JSONSerialization.data(withJSONObject: valor, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!); exit(0)
}
let inicio = Date()
let args = Array(CommandLine.arguments.dropFirst())
if args == ["--permissoes"] {
    resposta(["acessibilidade": AXIsProcessTrusted(), "tela": telaPermitida()])
}
var bundle: String? = nil
if !args.isEmpty {
    guard args.count == 2, args[0] == "--app", !args[1].isEmpty else {
        resposta(["ok": false, "motivo": "uso: olhar-mac [--app bundle-id | --permissoes]"])
    }
    bundle = args[1]
}
let axOK = AXIsProcessTrusted(), telaOK = telaPermitida()
if !axOK && !telaOK {
    resposta(["ok": false, "motivo": "permissoes_necessarias", "permissoes": ["acessibilidade": false, "tela": false], "somente_leitura": true])
}
let app = bundle.flatMap { NSRunningApplication.runningApplications(withBundleIdentifier: $0).first }
    ?? (bundle == nil ? NSWorkspace.shared.frontmostApplication : nil)
guard let app = app else { resposta(["ok": false, "motivo": "app_nao_aberto"]) }
let pid = app.processIdentifier
var saida: [String: Any] = [
    "versao": 1, "capturado_em": ISO8601DateFormatter().string(from: inicio),
    "app": ["nome": app.localizedName ?? "", "bundle": app.bundleIdentifier ?? "", "pid": pid],
    "permissoes": ["acessibilidade": axOK, "tela": telaOK],
    "somente_leitura": true,
]
// Seleciona uma única janela. A foto e a árvore devem descrever a MESMA janela.
let janelas = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? [])
    .filter { ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) && ($0[kCGWindowLayer as String] as? Int) == 0 }
guard let janela = janelas.first,
      let numero = janela[kCGWindowNumber as String] as? UInt32,
      let limites = janela[kCGWindowBounds as String] as? [String: Any],
      let ret = CGRect(dictionaryRepresentation: limites as CFDictionary), ret.width > 0, ret.height > 0 else {
    saida["ok"] = false; saida["motivo"] = "sem_janela_visivel"; resposta(saida)
}
saida["janela"] = ["numero": numero, "titulo": janela[kCGWindowName as String] as? String ?? "", "ret": retJSON(ret)]
var avisos: [String] = [], controles: [[String: Any]] = []
var truncada = false, senha = false
var raiz: AXUIElement? = nil
if axOK {
    let axApp = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(axApp, 0.15)
    let candidatas = atributo(axApp, kAXWindowsAttribute) as? [AXUIElement] ?? []
    raiz = candidatas.first { el in
        guard let r = retangulo(el) else { return false }
        return abs(r.minX - ret.minX) < 2 && abs(r.minY - ret.minY) < 2 && abs(r.width - ret.width) < 2 && abs(r.height - ret.height) < 2
    }
    if let raiz = raiz {
        let limite = Date().addingTimeInterval(2)
        var fila: [(AXUIElement, Int?, Int)] = [(raiz, nil, 0)], vistos: [AXUIElement] = []
        var indice = 0
        while indice < fila.count {
            if controles.count >= 150 || Date() >= limite { truncada = true; break }
            let (el, pai, nivel) = fila[indice]; indice += 1
            if vistos.contains(where: { CFEqual($0, el) }) { continue }; vistos.append(el)
            let papel = texto(el, kAXRoleAttribute), sub = texto(el, kAXSubroleAttribute)
            let protegido = papel == "AXSecureTextField" || sub == "AXSecureTextField"
            senha = senha || protegido
            let ref = controles.count
            var c: [String: Any] = ["ref": ref, "papel": papel, "subpapel": sub]
            if let pai = pai { c["pai"] = pai }
            if let r = retangulo(el) { c["ret"] = retJSON(r) }
            if protegido { c["protegido"] = true }
            else {
                c["titulo"] = texto(el, kAXTitleAttribute); c["descricao"] = texto(el, kAXDescriptionAttribute)
                c["valor"] = texto(el, kAXValueAttribute)
            }
            controles.append(c)
            if protegido { continue }
            let filhos = atributo(el, kAXChildrenAttribute) as? [AXUIElement] ?? []
            if nivel >= 8 { if !filhos.isEmpty { truncada = true }; continue }
            let vagas = max(0, 300 - fila.count)
            if filhos.count > vagas { truncada = true }
            fila.append(contentsOf: filhos.prefix(vagas).map { ($0, ref, nivel + 1) })
        }
    } else { avisos.append("janela_sem_arvore_correspondente") }
} else { avisos.append("acessibilidade_nao_autorizada") }
saida["acessibilidade"] = ["estado": !axOK ? "sem-permissao" : raiz == nil ? "indisponivel" : truncada ? "parcial" : "ok", "truncada": truncada, "controles": controles]
if !telaOK { avisos.append("gravacao_de_tela_nao_autorizada") }
else if senha { avisos.append("foto_omitida_campo_protegido") }
else if let img = CGWindowListCreateImage(.null, .optionIncludingWindow, CGWindowID(numero), [.boundsIgnoreFraming, .nominalResolution]) {
    let escala = min(1, 1600 / CGFloat(max(img.width, img.height)))
    let w = max(1, Int(CGFloat(img.width) * escala)), h = max(1, Int(CGFloat(img.height) * escala))
    if let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) {
        ctx.interpolationQuality = .high; ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
        if let pequena = ctx.makeImage(), let jpg = NSBitmapImageRep(cgImage: pequena).representation(using: .jpeg, properties: [.compressionFactor: 0.7]) {
            saida["foto"] = ["mime": "image/jpeg", "dados": jpg.base64EncodedString(), "largura": w, "altura": h, "ret": retJSON(ret)]
        }
    }
}
if saida["foto"] == nil && telaOK && !senha { avisos.append("foto_indisponivel") }
// Uma janela mudando durante a leitura não é evidência confiável para agir depois.
let atuais = CGWindowListCopyWindowInfo(.optionIncludingWindow, CGWindowID(numero)) as? [[String: Any]] ?? []
let mudou = atuais.first.flatMap { $0[kCGWindowBounds as String] as? [String: Any] }.flatMap { CGRect(dictionaryRepresentation: $0 as CFDictionary) } != ret
saida["consistente"] = !mudou
if mudou { avisos.append("janela_mudou_durante_leitura"); saida.removeValue(forKey: "foto") }
saida["ok"] = !mudou && (saida["foto"] != nil || !controles.isEmpty)
saida["avisos"] = avisos
saida["duracao_ms"] = Int(Date().timeIntervalSince(inicio) * 1000)
resposta(saida)
