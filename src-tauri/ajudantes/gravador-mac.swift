// dn.os Desktop — gravador da máquina toda ("Aprenda comigo", fase 1, 13/09/2026).
//
// Ajudante em Swift, embutido na casca (include_bytes! em maquina.rs) e escrito
// em <app_data>/ajudantes/ na primeira vez. Só OBSERVA: nenhum clique sai daqui.
//
// Modos:
//   dnos-gravador-mac permissoes [--pedir]
//       Imprime {"acessibilidade":bool,"tela":bool}. Com --pedir, abre os
//       diálogos do macOS (Acessibilidade e Gravação de Tela).
//   dnos-gravador-mac gravar [--ignorar <bundleId>] [--sem-foto]
//       Escuta clique, duplo clique, arrasto, digitação, tecla, atalho, rolagem
//       e troca de app pelo CGEventTap (só escuta) e descreve o alvo pela
//       Acessibilidade (papel, título, valor, caminho até a janela) — a
//       coordenada é evidência, não alvo. Uma foto JPEG da janela ativa por
//       passo (mín. 1,2 s entre fotos, teto 80). Senha (AXSecureTextField)
//       vira "••••". Cada passo sai como UMA linha JSON no stdout. Lê o stdin:
//       "parar" encerra; EOF (a casca morreu) também.
//
// Compila com o Swift 5.3 / SDK 10.15 (CLT antigas): sem ScreenCaptureKit,
// foto pelo CGWindowListCreateImage.

import Cocoa
import ApplicationServices

// ───────────────────────── utilidades ─────────────────────────

func agoraMs() -> UInt64 { UInt64(Date().timeIntervalSince1970 * 1000) }

func linha(_ o: [String: Any]) {
    if let d = try? JSONSerialization.data(withJSONObject: o, options: []), let s = String(data: d, encoding: .utf8) {
        print(s)
        fflush(stdout)
    }
}

func diario(_ s: String) {
    FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
}

// ───────────────────────── permissões ─────────────────────────

func acessibilidadeOk(pedir: Bool) -> Bool {
    let chave = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
    return AXIsProcessTrustedWithOptions([chave: pedir] as CFDictionary)
}

/// CGPreflightScreenCaptureAccess / CGRequestScreenCaptureAccess por dlsym: o SDK
/// 10.15 das CLT antigas não os expõe ao Swift, e o binário precisa compilar nos dois.
func telaOk(pedir: Bool) -> Bool {
    typealias Fn = @convention(c) () -> Bool
    let nome = pedir ? "CGRequestScreenCaptureAccess" : "CGPreflightScreenCaptureAccess"
    guard let h = dlopen("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics", RTLD_LAZY), let sym = dlsym(h, nome) else { return true }
    return unsafeBitCast(sym, to: Fn.self)()
}

// ───────────────────────── acessibilidade ─────────────────────────

let sistema = AXUIElementCreateSystemWide()

func axAtributo(_ el: AXUIElement, _ nome: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(el, nome as CFString, &v) == .success ? v : nil
}

func axTexto(_ el: AXUIElement, _ nome: String) -> String {
    guard let v = axAtributo(el, nome) else { return "" }
    if let s = v as? String { return s }
    if let n = v as? NSNumber { return n.stringValue }
    if let u = v as? URL { return u.absoluteString }
    return ""
}

func axElemento(_ v: AnyObject?) -> AXUIElement? {
    guard let v = v, CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
    return (v as! AXUIElement)
}

func axRetangulo(_ el: AXUIElement) -> [String: Any]? {
    guard let p = axAtributo(el, kAXPositionAttribute), let s = axAtributo(el, kAXSizeAttribute) else { return nil }
    var pt = CGPoint.zero, sz = CGSize.zero
    guard CFGetTypeID(p) == AXValueGetTypeID(), CFGetTypeID(s) == AXValueGetTypeID() else { return nil }
    AXValueGetValue(p as! AXValue, .cgPoint, &pt)
    AXValueGetValue(s as! AXValue, .cgSize, &sz)
    return ["x": Int(pt.x), "y": Int(pt.y), "w": Int(sz.width), "h": Int(sz.height)]
}

func corta(_ s: String, _ n: Int = 120) -> String {
    let t = s.replacingOccurrences(of: "\n", with: " ").trimmingCharacters(in: .whitespacesAndNewlines)
    return t.count > n ? String(t.prefix(n)) + "…" : t
}

/// O que uma pessoa (e um agente) reconhece no elemento: papel, título,
/// descrição, valor (nunca de campo de senha), identificador e o caminho de
/// ancestrais até a janela ("AXWindow 'Exportar' > AXGroup > AXButton 'OK'").
func descrever(_ el: AXUIElement) -> [String: Any] {
    let papel = axTexto(el, kAXRoleAttribute)
    var d: [String: Any] = [
        "papel": papel,
        "subpapel": axTexto(el, kAXSubroleAttribute),
        "titulo": corta(axTexto(el, kAXTitleAttribute)),
        "descricao": corta(axTexto(el, kAXDescriptionAttribute)),
        "id": corta(axTexto(el, kAXIdentifierAttribute), 80),
        "ajuda": corta(axTexto(el, kAXHelpAttribute), 80),
    ]
    if papel == "AXSecureTextField" {
        d["senha"] = true
    } else {
        d["valor"] = corta(axTexto(el, kAXValueAttribute))
    }
    if let r = axRetangulo(el) { d["ret"] = r }
    var caminho: [String] = []
    var atual = el
    for _ in 0..<10 {
        guard let pai = axElemento(axAtributo(atual, kAXParentAttribute)) else { break }
        let pp = axTexto(pai, kAXRoleAttribute)
        if pp.isEmpty || pp == kAXApplicationRole as String { break }
        let t = corta(axTexto(pai, kAXTitleAttribute), 40)
        caminho.insert(t.isEmpty ? pp : "\(pp) '\(t)'", at: 0)
        if pp == kAXWindowRole as String { break }
        atual = pai
    }
    d["caminho"] = caminho
    return d
}

func elementoEm(_ x: CGFloat, _ y: CGFloat) -> AXUIElement? {
    var el: AXUIElement?
    return AXUIElementCopyElementAtPosition(sistema, Float(x), Float(y), &el) == .success ? el : nil
}

func appDaFrente() -> (pid: pid_t, nome: String, bundle: String)? {
    guard let a = NSWorkspace.shared.frontmostApplication else { return nil }
    return (a.processIdentifier, a.localizedName ?? "", a.bundleIdentifier ?? "")
}

func janelaFocada(_ pid: pid_t) -> (el: AXUIElement, titulo: String, ret: [String: Any]?)? {
    let app = AXUIElementCreateApplication(pid)
    guard let w = axElemento(axAtributo(app, kAXFocusedWindowAttribute)) else { return nil }
    return (w, corta(axTexto(w, kAXTitleAttribute), 100), axRetangulo(w))
}

func elementoFocado(_ pid: pid_t) -> AXUIElement? {
    let app = AXUIElementCreateApplication(pid)
    return axElemento(axAtributo(app, kAXFocusedUIElementAttribute))
}

// ───────────────────────── foto da janela ─────────────────────────

func idDaJanela(_ pid: pid_t) -> CGWindowID? {
    guard let lista = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else { return nil }
    for w in lista {
        guard let p = w[kCGWindowOwnerPID as String] as? Int, p == Int(pid) else { continue }
        guard let camada = w[kCGWindowLayer as String] as? Int, camada == 0 else { continue }
        if let n = w[kCGWindowNumber as String] as? Int { return CGWindowID(n) }
    }
    return nil
}

func fotoDaJanela(_ pid: pid_t) -> String? {
    guard let wid = idDaJanela(pid) else { return nil }
    guard let img = CGWindowListCreateImage(.null, .optionIncludingWindow, wid, [.boundsIgnoreFraming, .nominalResolution]) else { return nil }
    // Reduz para 1280 de largura no máximo: é registro, não impressão.
    let escala = min(1.0, 1280.0 / CGFloat(max(img.width, 1)))
    let lw = max(1, Int(CGFloat(img.width) * escala)), lh = max(1, Int(CGFloat(img.height) * escala))
    guard let ctx = CGContext(data: nil, width: lw, height: lh, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
    ctx.interpolationQuality = .medium
    ctx.draw(img, in: CGRect(x: 0, y: 0, width: lw, height: lh))
    guard let pequena = ctx.makeImage() else { return nil }
    let rep = NSBitmapImageRep(cgImage: pequena)
    guard let jpeg = rep.representation(using: .jpeg, properties: [.compressionFactor: 0.45]) else { return nil }
    return "data:image/jpeg;base64," + jpeg.base64EncodedString()
}

// ───────────────────────── gravação ─────────────────────────

final class Gravador {
    var ignorar: String = ""
    var comFoto = true
    var passos = 0
    var ultimaFotoMs: UInt64 = 0
    // digitação em curso
    var texto = ""
    var textoInicioMs: UInt64 = 0
    var textoContexto: [String: Any]? = nil
    var textoSenha = false
    // clique esperando um possível duplo
    var cliquePendente: [String: Any]? = nil
    var cliqueTimer: DispatchWorkItem? = nil
    // arrasto
    var descida: CGPoint? = nil
    var descidaMs: UInt64 = 0
    var descidaAx: [String: Any]? = nil
    // rolagem coalescida
    var rolagemMs: UInt64 = 0
    var rolagemDy: Double = 0
    var rolagemCtx: [String: Any]? = nil
    var ultimoBundle = ""
    let fila = DispatchQueue(label: "dnos.gravador")

    /// app + janela + coordenada + elemento sob o ponto — o contexto de qualquer passo.
    func contexto(_ ponto: CGPoint?, elemento: AXUIElement? = nil) -> [String: Any]? {
        guard let app = appDaFrente() else { return nil }
        if !ignorar.isEmpty && app.bundle == ignorar { return nil }   // a própria casca não é passo
        var c: [String: Any] = ["app": ["nome": app.nome, "bundle": app.bundle], "hora": agoraMs()]
        if let j = janelaFocada(app.pid) {
            c["janela"] = j.titulo
            if let p = ponto, let r = j.ret, let jx = r["x"] as? Int, let jy = r["y"] as? Int, let jw = r["w"] as? Int, let jh = r["h"] as? Int {
                c["coord"] = ["x": Int(p.x), "y": Int(p.y), "jx": jx, "jy": jy, "jw": jw, "jh": jh,
                              "rx": jw > 0 ? Double(Int(p.x) - jx) / Double(jw) : 0, "ry": jh > 0 ? Double(Int(p.y) - jy) / Double(jh) : 0]
            }
        } else if let p = ponto {
            c["coord"] = ["x": Int(p.x), "y": Int(p.y)]
        }
        let el = elemento ?? (ponto.flatMap { elementoEm($0.x, $0.y) })
        if let el = el { c["ax"] = descrever(el) }
        c["pid"] = Int(app.pid)
        return c
    }

    func emitir(_ passo: [String: Any]) {
        var p = passo
        let pid = (p.removeValue(forKey: "pid") as? Int).map { pid_t($0) }
        passos += 1
        p["n"] = passos
        if comFoto, let pid = pid, agoraMs() - ultimaFotoMs > 1200, passos <= 80 {
            ultimaFotoMs = agoraMs()
            if let f = fotoDaJanela(pid) { p["quadro"] = f }
        }
        linha(p)
    }

    // ── digitação: junta caracteres até uma pausa, um clique ou Enter ──
    func caractere(_ s: String, pid: pid_t) {
        if texto.isEmpty {
            textoInicioMs = agoraMs()
            let el = elementoFocado(pid)
            textoContexto = contexto(nil, elemento: el)
            textoSenha = (textoContexto?["ax"] as? [String: Any])?["senha"] as? Bool ?? false
            if textoContexto == nil { return }
        }
        texto += s
        agendarDescarga()
    }
    func apagar() {
        if !texto.isEmpty { texto.removeLast(); agendarDescarga() }
    }
    var descargaTimer: DispatchWorkItem? = nil
    func agendarDescarga() {
        descargaTimer?.cancel()
        let t = DispatchWorkItem { [weak self] in self?.descarregarTexto() }
        descargaTimer = t
        fila.asyncAfter(deadline: .now() + 1.5, execute: t)
    }
    func descarregarTexto() {
        descargaTimer?.cancel(); descargaTimer = nil
        guard !texto.isEmpty, var c = textoContexto else { texto = ""; textoContexto = nil; return }
        c["t"] = "digitar"
        c["hora"] = textoInicioMs
        c["valor"] = textoSenha ? "••••" : corta(texto, 200)
        if textoSenha { c["senha"] = true }
        texto = ""; textoContexto = nil; textoSenha = false
        emitir(c)
    }

    // ── clique: espera 350 ms por um duplo ──
    func clique(_ ponto: CGPoint, botao: String, contagem: Int, mods: [String]) {
        descarregarTexto()
        cliqueTimer?.cancel()
        guard var c = contexto(ponto) else { return }
        c["t"] = contagem >= 2 ? "duplo" : "clique"
        if botao != "esquerdo" { c["botao"] = botao }
        if !mods.isEmpty { c["mods"] = mods }
        if contagem >= 2 { cliquePendente = nil; emitir(c); return }
        cliquePendente = c
        let t = DispatchWorkItem { [weak self] in
            guard let s = self, let p = s.cliquePendente else { return }
            s.cliquePendente = nil; s.emitir(p)
        }
        cliqueTimer = t
        fila.asyncAfter(deadline: .now() + 0.35, execute: t)
    }

    func arrasto(de: CGPoint, para: CGPoint, axInicio: [String: Any]?) {
        cliqueTimer?.cancel(); cliquePendente = nil
        guard var c = contexto(para) else { return }
        c["t"] = "arrastar"
        c["de"] = ["x": Int(de.x), "y": Int(de.y)]
        if let a = axInicio { c["axDe"] = a }
        emitir(c)
    }

    func tecla(_ nome: String, pid: pid_t) {
        descarregarTexto()
        guard var c = contexto(nil, elemento: elementoFocado(pid)) else { return }
        c["t"] = "tecla"; c["tecla"] = nome
        emitir(c)
    }

    func atalho(_ combo: String, pid: pid_t) {
        descarregarTexto()
        guard var c = contexto(nil, elemento: elementoFocado(pid)) else { return }
        c["t"] = "atalho"; c["tecla"] = combo
        emitir(c)
    }

    func rolagem(_ ponto: CGPoint, dy: Double) {
        let agora = agoraMs()
        if agora - rolagemMs > 800 || rolagemCtx == nil {
            descarregarRolagem()
            rolagemCtx = contexto(ponto); rolagemDy = 0
        }
        rolagemMs = agora; rolagemDy += dy
        rolagemTimer?.cancel()
        let t = DispatchWorkItem { [weak self] in self?.descarregarRolagem() }
        rolagemTimer = t
        fila.asyncAfter(deadline: .now() + 0.9, execute: t)
    }
    var rolagemTimer: DispatchWorkItem? = nil
    func descarregarRolagem() {
        guard var c = rolagemCtx else { return }
        rolagemCtx = nil
        c["t"] = "rolar"; c["direcao"] = rolagemDy < 0 ? "baixo" : "cima"; c["quanto"] = Int(abs(rolagemDy))
        // Rolagem não merece foto: é preparação, não ação.
        var semFoto = c; semFoto.removeValue(forKey: "pid")
        passos += 1; semFoto["n"] = passos
        linha(semFoto)
    }

    func trocaDeApp(_ app: NSRunningApplication) {
        let b = app.bundleIdentifier ?? ""
        if b == ignorar || b == ultimoBundle { return }
        ultimoBundle = b
        descarregarTexto()
        var c: [String: Any] = ["t": "app", "hora": agoraMs(), "app": ["nome": app.localizedName ?? "", "bundle": b], "pid": Int(app.processIdentifier)]
        if let j = janelaFocada(app.processIdentifier) { c["janela"] = j.titulo }
        emitir(c)
    }

    func encerrar() {
        descarregarTexto(); descarregarRolagem()
        if let p = cliquePendente { cliquePendente = nil; emitir(p) }
    }
}

let gravador = Gravador()

// Nomes das teclas especiais pelo keycode (layout ANSI/ABNT; letras vêm pelo unicode).
let TECLAS: [Int64: String] = [36: "Enter", 76: "Enter", 48: "Tab", 53: "Escape", 51: "Backspace", 117: "Delete",
                              123: "Esquerda", 124: "Direita", 125: "Baixo", 126: "Cima", 115: "Home", 119: "End", 116: "PageUp", 121: "PageDown",
                              122: "F1", 120: "F2", 99: "F3", 118: "F4", 96: "F5", 97: "F6", 98: "F7", 100: "F8", 101: "F9", 109: "F10", 103: "F11", 111: "F12"]

func modificadores(_ f: CGEventFlags) -> [String] {
    var m: [String] = []
    if f.contains(.maskCommand) { m.append("cmd") }
    if f.contains(.maskAlternate) { m.append("opt") }
    if f.contains(.maskControl) { m.append("ctrl") }
    if f.contains(.maskShift) { m.append("shift") }
    return m
}

func tratar(_ tipo: CGEventType, _ ev: CGEvent) {
    guard let app = appDaFrente() else { return }
    if !gravador.ignorar.isEmpty && app.bundle == gravador.ignorar { return }
    let ponto = ev.location
    switch tipo {
    case .leftMouseDown:
        gravador.descida = ponto; gravador.descidaMs = agoraMs()
        gravador.descidaAx = elementoEm(ponto.x, ponto.y).map { descrever($0) }
    case .leftMouseUp:
        let contagem = Int(ev.getIntegerValueField(.mouseEventClickState))
        if let d = gravador.descida, hypot(ponto.x - d.x, ponto.y - d.y) > 8 {
            gravador.arrasto(de: d, para: ponto, axInicio: gravador.descidaAx)
        } else {
            gravador.clique(ponto, botao: "esquerdo", contagem: contagem, mods: modificadores(ev.flags))
        }
        gravador.descida = nil; gravador.descidaAx = nil
    case .rightMouseDown:
        gravador.clique(ponto, botao: "direito", contagem: 1, mods: modificadores(ev.flags))
    case .scrollWheel:
        let dy = Double(ev.getIntegerValueField(.scrollWheelEventPointDeltaAxis1))
        if dy != 0 { gravador.rolagem(ponto, dy: dy) }
    case .keyDown:
        let code = ev.getIntegerValueField(.keyboardEventKeycode)
        let f = ev.flags
        let mods = modificadores(f)
        var chars = [UniChar](repeating: 0, count: 4)
        var n = 0
        ev.keyboardGetUnicodeString(maxStringLength: 4, actualStringLength: &n, unicodeString: &chars)
        let s = String(utf16CodeUnits: chars, count: n)
        if f.contains(.maskCommand) || f.contains(.maskControl) {
            let nome = TECLAS[code] ?? s.uppercased()
            gravador.atalho((mods + [nome]).joined(separator: "+"), pid: app.pid)
        } else if let nome = TECLAS[code] {
            if nome == "Backspace" { gravador.apagar() }
            else if nome == "Enter" || nome == "Tab" || nome == "Escape" || nome == "Delete" { gravador.tecla(nome, pid: app.pid) }
            else { gravador.tecla(nome, pid: app.pid) }
        } else if !s.isEmpty, let u = s.unicodeScalars.first, u.value >= 32 {
            gravador.caractere(s, pid: app.pid)
        }
    default:
        break
    }
}

var tapGlobal: CFMachPort? = nil

func gravar() {
    let mascara: CGEventMask =
        (1 << CGEventType.leftMouseDown.rawValue) | (1 << CGEventType.leftMouseUp.rawValue) |
        (1 << CGEventType.rightMouseDown.rawValue) | (1 << CGEventType.keyDown.rawValue) |
        (1 << CGEventType.scrollWheel.rawValue)
    let callback: CGEventTapCallBack = { _, tipo, ev, _ in
        if tipo == .tapDisabledByTimeout || tipo == .tapDisabledByUserInput {
            if let t = tapGlobal { CGEvent.tapEnable(tap: t, enable: true) }
            return Unmanaged.passUnretained(ev)
        }
        // Copia o que precisa fora da fila do sistema: o tap não pode demorar.
        let copia = ev.copy()
        gravador.fila.async { if let c = copia { tratar(tipo, c) } }
        return Unmanaged.passUnretained(ev)
    }
    guard let tap = CGEvent.tapCreate(tap: .cghidEventTap, place: .headInsertEventTap, options: .listenOnly, eventsOfInterest: mascara, callback: callback, userInfo: nil) else {
        linha(["t": "erro", "motivo": "não consegui escutar mouse e teclado: sem permissão de Acessibilidade"])
        exit(2)
    }
    tapGlobal = tap
    let fonte = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
    CFRunLoopAddSource(CFRunLoopGetCurrent(), fonte, .commonModes)
    CGEvent.tapEnable(tap: tap, enable: true)

    // Troca de app é um passo ("No CapCut…").
    NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: nil) { n in
        if let a = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication {
            gravador.fila.async { gravador.trocaDeApp(a) }
        }
    }
    if let a = NSWorkspace.shared.frontmostApplication { gravador.ultimoBundle = a.bundleIdentifier ?? "" }

    linha(["t": "pronto", "hora": agoraMs(), "acessibilidade": acessibilidadeOk(pedir: false), "tela": telaOk(pedir: false)])

    // stdin: "parar" encerra; EOF = a casca sumiu.
    DispatchQueue.global().async {
        while let l = readLine() {
            if l.trimmingCharacters(in: .whitespacesAndNewlines) == "parar" { break }
        }
        gravador.fila.sync { gravador.encerrar() }
        linha(["t": "fim", "hora": agoraMs(), "passos": gravador.passos])
        exit(0)
    }
    signal(SIGTERM) { _ in
        gravador.fila.sync { gravador.encerrar() }
        linha(["t": "fim", "hora": agoraMs(), "passos": gravador.passos])
        exit(0)
    }
    CFRunLoopRun()
}

// ───────────────────────── main ─────────────────────────

let args = CommandLine.arguments.dropFirst()
let modo = args.first ?? "permissoes"
if modo == "permissoes" {
    let pedir = args.contains("--pedir")
    let ax = acessibilidadeOk(pedir: pedir)
    let tela = telaOk(pedir: pedir)
    linha(["acessibilidade": ax, "tela": tela])
    exit(0)
}
if modo == "gravar" {
    if let i = args.firstIndex(of: "--ignorar"), let b = args.dropFirst(i - args.startIndex + 1).first { gravador.ignorar = b }
    gravador.comFoto = !args.contains("--sem-foto")
    gravar()
}
linha(["t": "erro", "motivo": "modo desconhecido: \(modo)"])
exit(1)
