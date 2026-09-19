// dn.os · fase 2C-1. Leitura pontual: não instala event tap, não ativa apps,
// não envia entrada e não pede permissões. Pode ser validada antes da integração.
import Cocoa
import ApplicationServices
import CryptoKit

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
if args == ["--soltar"] {
    let pt=CGEvent(source:nil)?.location ?? .zero
    CGEvent(mouseEventSource:nil,mouseType:.leftMouseUp,mouseCursorPosition:pt,mouseButton:.left)?.post(tap:.cghidEventTap)
    for key: CGKeyCode in [0,53,36,48,49,123,124,125,126,51] { CGEvent(keyboardEventSource:nil,virtualKey:key,keyDown:false)?.post(tap:.cghidEventTap) }
    resposta(["ok":true])
}
if args == ["--apps"] {
    let apps = NSWorkspace.shared.runningApplications.filter { $0.activationPolicy == .regular && $0.bundleIdentifier != nil }
        .map { ["bundle": $0.bundleIdentifier!, "nome": $0.localizedName ?? $0.bundleIdentifier!] }
    resposta(["apps": apps.sorted { $0["nome"]! < $1["nome"]! }])
}
var pedido: [String: Any]? = nil
if args == ["--acao"] {
    let dados = FileHandle.standardInput.readData(ofLength: 16385)
    guard dados.count <= 16384, let v = try? JSONSerialization.jsonObject(with: dados) as? [String: Any] else { resposta(["ok":false,"motivo":"acao_invalida"]) }
    pedido = v
}
var bundle: String? = pedido?["bundle"] as? String
if !args.isEmpty && pedido == nil {
    guard args.count == 2, ["--app","--explorar"].contains(args[0]), !args[1].isEmpty else {
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
// A autorização de sessão permite trazer SOMENTE o app escolhido para frente.
if pedido != nil || args.first == "--explorar" {
    guard axOK && telaOK else { resposta(["ok":false,"motivo":"permissoes_necessarias"]) }
    app.activate(options: [.activateIgnoringOtherApps])
    // 18/09: NSWorkspace.frontmostApplication é cache e, num processo sem
    // NSApplication, não se atualiza — o mesmo furo que fez o roteiro acusar
    // "não consegui abrir CapCut" com o CapCut na frente. Pergunta ao próprio
    // app (isActive) e ao servidor de janelas, por até 2 s.
    func naFrente() -> Bool {
        if app.isActive { return true }
        let lista = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        for w in lista where (w[kCGWindowLayer as String] as? Int) == 0 { return (w[kCGWindowOwnerPID as String] as? pid_t) == pid }
        return false
    }
    var chegou = false
    for _ in 0..<10 { if naFrente() { chegou = true; break }; Thread.sleep(forTimeInterval: 0.2) }
    guard chegou else { resposta(["ok":false,"motivo":"app_nao_esta_na_frente"]) }
}
var saida: [String: Any] = [
    "versao": 1, "capturado_em": ISO8601DateFormatter().string(from: inicio),
    "app": ["nome": app.localizedName ?? "", "bundle": app.bundleIdentifier ?? "", "pid": pid],
    "permissoes": ["acessibilidade": axOK, "tela": telaOK],
    "somente_leitura": true,
]
// Seleciona uma única janela. A foto e a árvore devem descrever a MESMA janela.
let janelas = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? [])
    .filter { ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) && ($0[kCGWindowLayer as String] as? Int) == 0 }
// 18/09: a primeira da lista é a mais à frente, e no CapCut era um painel
// flutuante de 183×88 ("EditPilot") por cima da janela do projeto. Ordem:
// a janela principal que o próprio app declara (AX); senão a maior que não
// seja painel pequeno; senão a da frente.
func limitesDe(_ w: [String: Any]) -> CGRect? {
    (w[kCGWindowBounds as String] as? [String: Any]).flatMap { CGRect(dictionaryRepresentation: $0 as CFDictionary) }
}
let principalAX: CGRect? = axOK ? {
    let axApp = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(axApp, 0.15)
    return atributo(axApp, kAXMainWindowAttribute).flatMap { retangulo($0 as! AXUIElement) }
}() : nil
let janelaEscolhida: [String: Any]? =
    principalAX.flatMap { m in janelas.first { w in limitesDe(w).map { abs($0.minX - m.minX) < 2 && abs($0.minY - m.minY) < 2 && abs($0.width - m.width) < 2 && abs($0.height - m.height) < 2 } ?? false } }
    ?? janelas.filter { w in limitesDe(w).map { $0.width >= 300 && $0.height >= 200 } ?? false }
        .max { (limitesDe($0).map { $0.width * $0.height } ?? 0) < (limitesDe($1).map { $0.width * $0.height } ?? 0) }
    ?? janelas.first
guard let janela = janelaEscolhida,
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
    if let bytes = img.dataProvider?.data {
        saida["impressao"] = SHA256.hash(data: bytes as Data).map { String(format: "%02x", $0) }.joined()
    }
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
if let pedido = pedido {
    guard !mudou, !senha, let esperado = pedido["impressao"] as? String,
          saida["impressao"] as? String == esperado,
          pedido["janela"] as? UInt32 == numero else { resposta(["ok":false,"motivo":"tela_mudou_observe_novamente"]) }
    let t = pedido["tipo"] as? String ?? ""
    func ponto(_ x: String, _ y: String) -> CGPoint? {
        guard let a = pedido[x] as? Double, let b = pedido[y] as? Double,
              a.isFinite, b.isFinite, a >= 0, a <= 1, b >= 0, b <= 1 else { return nil }
        return CGPoint(x:ret.minX+a*ret.width,y:ret.minY+b*ret.height)
    }
    func conferir(_ pt: CGPoint) -> Bool {
        var alvo: AXUIElement?
        guard AXUIElementCopyElementAtPosition(AXUIElementCreateSystemWide(),Float(pt.x),Float(pt.y),&alvo) == .success, let alvo=alvo else { return false }
        var dono: pid_t = 0; AXUIElementGetPid(alvo,&dono)
        return dono == pid && texto(alvo,kAXRoleAttribute) != "AXSecureTextField" && texto(alvo,kAXSubroleAttribute) != "AXSecureTextField"
    }
    // Mesmo gesto do roteiro (gravador-mac `clicarEm`), que clica no CapCut: cursor
    // chega antes, e o botão desce e sobe com intervalo e contagem de clique.
    // 18/09: descer+subir no mesmo instante só realçava o Exportar, sem abrir nada.
    func mouse(_ tipo: CGEventType,_ pt: CGPoint) {
        guard let ev = CGEvent(mouseEventSource:nil,mouseType:tipo,mouseCursorPosition:pt,mouseButton:.left) else { return }
        if tipo == .leftMouseDown || tipo == .leftMouseUp { ev.setIntegerValueField(.mouseEventClickState, value: 1) }
        ev.post(tap:.cghidEventTap)
    }
    if t == "clique" || t == "arrastar" {
        guard let pt=ponto("x","y"),conferir(pt) else { resposta(["ok":false,"motivo":"alvo_fora_do_app_ou_protegido"]) }
        var fim: CGPoint? = nil
        if t == "arrastar" { fim=ponto("x2","y2"); guard let f=fim,conferir(f) else { resposta(["ok":false,"motivo":"destino_fora_do_app"]) } }
        mouse(.mouseMoved,pt); Thread.sleep(forTimeInterval:0.06)
        mouse(.leftMouseDown,pt); Thread.sleep(forTimeInterval:0.04)
        if let f=fim { for i in 1...12 { mouse(.leftMouseDragged,CGPoint(x:pt.x+(f.x-pt.x)*Double(i)/12,y:pt.y+(f.y-pt.y)*Double(i)/12)); Thread.sleep(forTimeInterval:0.015) } }
        mouse(.leftMouseUp,fim ?? pt)
    } else if t == "rolar" {
        guard let pt=ponto("x","y"),conferir(pt),let dy=pedido["dy"] as? Int,abs(dy)<=600 else { resposta(["ok":false,"motivo":"rolagem_invalida"]) }
        let ev=CGEvent(scrollWheelEvent2Source:nil,units:.pixel,wheelCount:1,wheel1:Int32(dy),wheel2:0,wheel3:0); ev?.location=pt;ev?.post(tap:.cghidEventTap)
    } else if t == "texto" || t == "tecla" {
        let axApp=AXUIElementCreateApplication(pid)
        guard let foco=elemento(atributo(axApp,kAXFocusedUIElementAttribute)),texto(foco,kAXRoleAttribute) != "AXSecureTextField",texto(foco,kAXSubroleAttribute) != "AXSecureTextField" else { resposta(["ok":false,"motivo":"foco_protegido_ou_indisponivel"]) }
        if t == "texto" {
            guard let valor=pedido["texto"] as? String,valor.utf16.count<=1000,!valor.contains("\n"),!valor.contains("\r") else { resposta(["ok":false,"motivo":"texto_invalido"]) }
            let chars=Array(valor.utf16)
            for baixo in [true,false] { let ev=CGEvent(keyboardEventSource:nil,virtualKey:0,keyDown:baixo);ev?.keyboardSetUnicodeString(stringLength:chars.count,unicodeString:chars);ev?.post(tap:.cghidEventTap) }
        } else {
            let teclas: [String:CGKeyCode] = ["escape":53,"enter":36,"tab":48,"espaco":49,"esquerda":123,"direita":124,"cima":126,"baixo":125,"backspace":51]
            guard let nome=pedido["tecla"] as? String,let tecla=teclas[nome] else { resposta(["ok":false,"motivo":"tecla_nao_permitida"]) }
            for baixo in [true,false] { CGEvent(keyboardEventSource:nil,virtualKey:tecla,keyDown:baixo)?.post(tap:.cghidEventTap) }
        }
    } else { resposta(["ok":false,"motivo":"acao_nao_permitida"]) }
    resposta(["ok":true,"executada":true])
}
resposta(saida)
