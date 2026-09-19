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

import AVFoundation
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

/// Abre um painel de Privacidade e Segurança e ESPERA o `open` terminar: com
/// NSWorkspace.open o ajudante saía antes de o pedido ser despachado (13/09).
func abrirPainel(_ ancora: String) {
    let p = Process()
    p.launchPath = "/usr/bin/open"
    p.arguments = ["x-apple.systempreferences:com.apple.preference.security?" + ancora]
    p.launch(); p.waitUntilExit()
}

/// Microfone (notas por voz, 13/09): sem autorização o macOS entrega silêncio
/// absoluto, sem erro — a casca via "sem sinal" e culpava o dispositivo.
func microfoneOk(pedir: Bool) -> Bool {
    let estado = AVCaptureDevice.authorizationStatus(for: .audio)
    if estado == .authorized { return true }
    if !pedir { return false }
    if estado == .notDetermined {
        let sem = DispatchSemaphore(value: 0)
        var ok = false
        AVCaptureDevice.requestAccess(for: .audio) { r in ok = r; sem.signal() }
        _ = sem.wait(timeout: .now() + 120)
        return ok
    }
    abrirPainel("Privacy_Microphone")
    return false
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
    if papel == "AXSecureTextField" || d["subpapel"] as? String == "AXSecureTextField" {
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

/// PID do app dono da janela comum (camada 0) mais ao topo, perguntado ao
/// servidor de janelas na hora. Não precisa de permissão de Gravação de Tela:
/// o dono da janela vem mesmo sem ela (o título, não).
func pidDaJanelaDoTopo() -> pid_t? {
    let lista = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    for w in lista where (w[kCGWindowLayer as String] as? Int) == 0 {
        if let pid = w[kCGWindowOwnerPID as String] as? pid_t, pid != getpid() { return pid }
    }
    return nil
}

/// Qual app está na frente AGORA (18/09).
///
/// `NSWorkspace.frontmostApplication` é um valor em cache que só se atualiza
/// com notificações do AppKit — neste processo (sem NSApplication, trabalho
/// numa fila de fundo esperando com usleep) ele NUNCA muda. O roteiro pedia o
/// CapCut, o CapCut vinha para a frente e o ajudante continuava vendo o app
/// anterior: 30 s de espera e "não consegui abrir CapCut" (Cora, 18/09 12:18 e
/// 16:17). Provado com réplica: pelo cache, nunca; pelo servidor de janelas e
/// por `isActive`, em 250 ms. O cache fica só como última reserva.
func appDaFrente() -> (pid: pid_t, nome: String, bundle: String)? {
    if let pid = pidDaJanelaDoTopo(), let a = NSRunningApplication(processIdentifier: pid) {
        return (pid, a.localizedName ?? "", a.bundleIdentifier ?? "")
    }
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
    var fotos = 0
    var ultimoQuadro: String? = nil
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
        let protegido = p["senha"] as? Bool == true || (p["ax"] as? [String: Any])?["senha"] as? Bool == true
        if comFoto, !protegido, let pid = pid, agoraMs() - ultimaFotoMs > 1200, fotos < 80 {
            ultimaFotoMs = agoraMs()
            if let f = fotoDaJanela(pid) { p["quadro"] = f; ultimoQuadro = f; fotos += 1 }
        }
        linha(p)
    }

    // Também registra mudanças enquanto a pessoa explica sem clicar. Quadros
    // idênticos não gastam outra interpretação; o teto é de fotos, não ações.
    func observar() {
        guard comFoto, fotos < 80, var c = contexto(nil), let pid = c["pid"] as? Int else { return }
        if let el = elementoFocado(pid_t(pid)), descrever(el)["senha"] as? Bool == true { return }
        guard let foto = fotoDaJanela(pid_t(pid)), foto != ultimoQuadro else { return }
        ultimaFotoMs = agoraMs(); ultimoQuadro = foto; fotos += 1
        c["t"] = "observar"; c["quadro"] = foto
        emitir(c)
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
        observar()
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

    let observacao = DispatchSource.makeTimerSource(queue: gravador.fila)
    observacao.schedule(deadline: .now() + 1, repeating: 5)
    observacao.setEventHandler { gravador.observar() }
    observacao.resume()

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
    withExtendedLifetime(observacao) { CFRunLoopRun() }
}


// ───────────────────────── execução (fase 2) ─────────────────────────
//
//   dnos-gravador-mac executar --roteiro <arquivo.json> [--ignorar <bundleId>]
//
// Lê {nome, passos:[…]} (os passos da gravação da máquina) e executa no Mac da
// pessoa, sem modelo: acha o alvo pela Acessibilidade (papel + título/descrição/
// id/valor, com o caminho de ancestrais como desempate; coordenada relativa à
// janela como último recurso), clica (AXPress quando o elemento aceita, senão
// mouse de verdade), digita (AXValue quando é campo, senão teclado), tecla,
// atalho, rolagem, arrasto e troca de app. Depois de cada passo espera a tela
// mudar (o alvo do próximo passo aparecer, até 8 s). Passo que não bate → para
// e devolve {t:"parou", passo, motivo, app, janela, foto}. Progresso: uma linha
// JSON por passo. Parar: "parar" no stdin, SIGTERM, ou Esc duas vezes seguidas.

final class Execucao {
    var parar = false
    let trava = NSLock()
    func pediuParar() -> Bool { trava.lock(); defer { trava.unlock() }; return parar }
    func mandarParar() { trava.lock(); parar = true; trava.unlock() }
}
let execucao = Execucao()

func dormir(_ ms: Int) { usleep(useconds_t(max(0, ms) * 1000)) }

func norm(_ s: String?) -> String {
    return (s ?? "").replacingOccurrences(of: "\u{2026}", with: "").replacingOccurrences(of: "\\s+", with: " ", options: .regularExpression).trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
}

/// Processos do sistema que aparecem como "app da frente" na gravação mas não
/// se abrem como app: o passo anterior (atalho ⌘Espaço, clique no Dock) já os
/// chamou. Na execução só se espera por eles, sem falhar (13/09).
let BUNDLES_DO_SISTEMA: Set<String> = ["com.apple.Spotlight", "com.apple.dock", "com.apple.controlcenter", "com.apple.notificationcenterui", "com.apple.loginwindow", "com.apple.systemuiserver"]

/// Ativa (ou abre) o app pelo bundle id e espera ficar na frente.
/// Abertura fria pode levar bem mais que 10 s (13/09): espera até 30 s.
func ativarApp(bundle: String, nome: String) -> Bool {
    if bundle.isEmpty { return false }
    if BUNDLES_DO_SISTEMA.contains(bundle) {
        for _ in 0..<16 { if let f = appDaFrente(), f.bundle == bundle { dormir(200); return true }; dormir(250) }
        return true
    }
    if let a = NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first {
        a.activate(options: [.activateIgnoringOtherApps])
    } else {
        if !NSWorkspace.shared.launchApplication(withBundleIdentifier: bundle, options: [], additionalEventParamDescriptor: nil, launchIdentifier: nil) { return false }
    }
    for _ in 0..<120 {
        if let f = appDaFrente(), f.bundle == bundle { dormir(250); return true }
        // O próprio app diz se está ativo (valor fresco, ao contrário do cache
        // do NSWorkspace) — cobre app ativo com a janela ainda minimizada.
        if NSRunningApplication.runningApplications(withBundleIdentifier: bundle).contains(where: { $0.isActive }) { dormir(250); return true }
        dormir(250)
    }
    return false
}

/// Clique num item do Dock (gravado com o app anterior na frente): o elemento é
/// do Dock, não do app. Acha pelo AX do Dock e aperta (13/09).
func clicarNoDock(_ ax: [String: Any]) -> Bool {
    guard let dock = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock").first else { return false }
    let pid = dock.processIdentifier
    let fim = Date().addingTimeInterval(4)
    repeat {
        if let (el, _) = acharElemento(alvo: ax, pid: pid) {
            if AXUIElementPerformAction(el, kAXPressAction as CFString) == .success { return true }
            if let c = centro(el) { clicarEm(c); return true }
        }
        dormir(250)
    } while Date() < fim
    return false
}

/// Percorre a árvore de acessibilidade do app da frente e devolve o elemento que
/// mais parece com o alvo gravado. Pontuação: papel obrigatório; título, descrição,
/// identificador, valor (texto estático) e caminho de ancestrais somam.
func acharElemento(alvo: [String: Any], pid: pid_t) -> (AXUIElement, Int)? {
    let papel = alvo["papel"] as? String ?? ""
    let subpapel = alvo["subpapel"] as? String ?? ""
    let titulo = norm(alvo["titulo"] as? String), desc = norm(alvo["descricao"] as? String)
    let ident = norm(alvo["id"] as? String), valor = norm(alvo["valor"] as? String)
    let caminho = (alvo["caminho"] as? [String]) ?? []
    let ultimoPai = norm(caminho.last)
    if papel.isEmpty { return nil }
    let app = AXUIElementCreateApplication(pid)
    var raizes: [AXUIElement] = []
    if let w = axElemento(axAtributo(app, kAXFocusedWindowAttribute)) { raizes.append(w) }
    if let ws = axAtributo(app, kAXWindowsAttribute) as? [AnyObject] { for w in ws { if let e = axElemento(w) { raizes.append(e) } } }
    // menus abertos ficam fora das janelas
    if let mb = axElemento(axAtributo(app, kAXMenuBarAttribute)) { raizes.append(mb) }
    var fila: [(AXUIElement, Int, String)] = raizes.map { ($0, 0, "") }
    var melhor: (AXUIElement, Int)? = nil
    var visitados = 0
    while !fila.isEmpty && visitados < 6000 {
        let (el, prof, pai) = fila.removeFirst(); visitados += 1
        let p = axTexto(el, kAXRoleAttribute)
        if p == papel {
            var pontos = 10
            let t = norm(axTexto(el, kAXTitleAttribute)), d = norm(axTexto(el, kAXDescriptionAttribute)), i = norm(axTexto(el, kAXIdentifierAttribute))
            if !ident.isEmpty && i == ident { pontos += 90 }
            if !titulo.isEmpty { if t == titulo { pontos += 100 } else if !t.isEmpty && (t.contains(titulo) || titulo.contains(t)) { pontos += 40 } }
            if !desc.isEmpty { if d == desc { pontos += 80 } else if !d.isEmpty && (d.contains(desc) || desc.contains(d)) { pontos += 30 } }
            if !valor.isEmpty && p == "AXStaticText" { let v = norm(axTexto(el, kAXValueAttribute)); if v == valor { pontos += 70 } else if v.contains(valor) { pontos += 25 } }
            if !subpapel.isEmpty && axTexto(el, kAXSubroleAttribute) == subpapel { pontos += 10 }
            if !ultimoPai.isEmpty && norm(pai).hasPrefix(ultimoPai.components(separatedBy: " '").first ?? "") { pontos += 5 }
            if axRetangulo(el) == nil { pontos -= 50 }
            let temNome = !titulo.isEmpty || !desc.isEmpty || !ident.isEmpty || !valor.isEmpty
            if (!temNome && pontos >= 10) || pontos >= 40 {
                if melhor == nil || pontos > melhor!.1 { melhor = (el, pontos) }
            }
        }
        if prof < 28, let filhos = axAtributo(el, kAXChildrenAttribute) as? [AnyObject] {
            let rotuloPai = p + " '" + corta(axTexto(el, kAXTitleAttribute), 40) + "'"
            for f in filhos { if let fe = axElemento(f) { fila.append((fe, prof + 1, rotuloPai)) } }
        }
    }
    return melhor
}

func centro(_ el: AXUIElement) -> CGPoint? {
    guard let r = axRetangulo(el), let x = r["x"] as? Int, let y = r["y"] as? Int, let w = r["w"] as? Int, let h = r["h"] as? Int, w > 0, h > 0 else { return nil }
    return CGPoint(x: Double(x) + Double(w) / 2, y: Double(y) + Double(h) / 2)
}

func mover(_ p: CGPoint) {
    CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
}

func clicarEm(_ p: CGPoint, direito: Bool = false, duplo: Bool = false) {
    mover(p); dormir(60)
    let down: CGEventType = direito ? .rightMouseDown : .leftMouseDown, up: CGEventType = direito ? .rightMouseUp : .leftMouseUp
    let botao: CGMouseButton = direito ? .right : .left
    for n in 1...(duplo ? 2 : 1) {
        if let d = CGEvent(mouseEventSource: nil, mouseType: down, mouseCursorPosition: p, mouseButton: botao) { d.setIntegerValueField(.mouseEventClickState, value: Int64(n)); d.post(tap: .cghidEventTap) }
        dormir(40)
        if let u = CGEvent(mouseEventSource: nil, mouseType: up, mouseCursorPosition: p, mouseButton: botao) { u.setIntegerValueField(.mouseEventClickState, value: Int64(n)); u.post(tap: .cghidEventTap) }
        if duplo { dormir(90) }
    }
}

let TECLA_POR_NOME: [String: CGKeyCode] = ["enter": 36, "return": 36, "tab": 48, "escape": 53, "esc": 53, "backspace": 51, "delete": 117, "espaco": 49, "space": 49,
    "esquerda": 123, "direita": 124, "baixo": 125, "cima": 126, "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111,
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17,
    "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35,
    "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47]

func pressionar(_ codigo: CGKeyCode, flags: CGEventFlags = []) {
    if let d = CGEvent(keyboardEventSource: nil, virtualKey: codigo, keyDown: true) { d.flags = flags; d.post(tap: .cghidEventTap) }
    dormir(30)
    if let u = CGEvent(keyboardEventSource: nil, virtualKey: codigo, keyDown: false) { u.flags = flags; u.post(tap: .cghidEventTap) }
}

func teclaPorNome(_ nome: String) -> Bool {
    guard let c = TECLA_POR_NOME[nome.lowercased()] else { return false }
    pressionar(c); return true
}

/// "cmd+shift+S" → flags + tecla.
func atalhoPorNome(_ combo: String) -> Bool {
    var flags = CGEventFlags()
    var tecla = ""
    for parte in combo.split(separator: "+").map({ $0.trimmingCharacters(in: .whitespaces) }) {
        switch parte.lowercased() {
        case "cmd", "command", "⌘": flags.insert(.maskCommand)
        case "opt", "option", "alt", "⌥": flags.insert(.maskAlternate)
        case "ctrl", "control", "⌃": flags.insert(.maskControl)
        case "shift", "⇧": flags.insert(.maskShift)
        default: tecla = parte
        }
    }
    guard let c = TECLA_POR_NOME[tecla.lowercased()] else { return false }
    pressionar(c, flags: flags); return true
}

func digitarTexto(_ texto: String) {
    for ch in texto {
        let u = Array(String(ch).utf16)
        if let d = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true) { d.keyboardSetUnicodeString(stringLength: u.count, unicodeString: u); d.post(tap: .cghidEventTap) }
        if let up = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false) { up.keyboardSetUnicodeString(stringLength: u.count, unicodeString: u); up.post(tap: .cghidEventTap) }
        dormir(12)
    }
}

func rolarEm(_ p: CGPoint, direcao: String, quanto: Int) {
    mover(p)
    let passos = max(1, min(30, quanto / 60))
    for _ in 0..<passos {
        if let e = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: Int32(direcao == "cima" ? 60 : -60), wheel2: 0, wheel3: 0) { e.post(tap: .cghidEventTap) }
        dormir(25)
    }
}

func arrastarDe(_ a: CGPoint, para b: CGPoint) {
    mover(a); dormir(80)
    CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: a, mouseButton: .left)?.post(tap: .cghidEventTap)
    dormir(120)
    for i in 1...12 {
        let t = CGFloat(i) / 12.0
        let p = CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t)
        CGEvent(mouseEventSource: nil, mouseType: .leftMouseDragged, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
        dormir(30)
    }
    dormir(80)
    CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: b, mouseButton: .left)?.post(tap: .cghidEventTap)
}

func fraseDoPasso(_ p: [String: Any]) -> String {
    if let d = p["descricao"] as? String, !d.isEmpty { return d }
    let ax = p["ax"] as? [String: Any]
    let nome = (ax?["titulo"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? (ax?["descricao"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? (ax?["valor"] as? String) ?? ""
    switch p["t"] as? String ?? "" {
    case "app": return "indo para " + ((p["app"] as? [String: Any])?["nome"] as? String ?? "o app")
    case "clique": return "clicando em \"\(nome)\""
    case "duplo": return "duplo clique em \"\(nome)\""
    case "arrastar": return "arrastando"
    case "digitar": return "digitando em \"\(nome)\""
    case "tecla": return "pressionando \(p["tecla"] as? String ?? "")"
    case "atalho": return "atalho \(p["tecla"] as? String ?? "")"
    case "rolar": return "rolando para \(p["direcao"] as? String ?? "baixo")"
    default: return p["t"] as? String ?? ""
    }
}

/// Coordenada gravada relativa à janela → ponto de tela na janela atual (último recurso).
func pontoRelativo(_ p: [String: Any], pid: pid_t) -> CGPoint? {
    guard let c = p["coord"] as? [String: Any], let rx = c["rx"] as? Double, let ry = c["ry"] as? Double else { return nil }
    guard let j = janelaFocada(pid), let r = j.ret, let jx = r["x"] as? Int, let jy = r["y"] as? Int, let jw = r["w"] as? Int, let jh = r["h"] as? Int, jw > 0, jh > 0 else { return nil }
    // Só vale se a janela tem tamanho parecido com o da gravação (±25%).
    if let gw = c["jw"] as? Int, let gh = c["jh"] as? Int, gw > 0, gh > 0 {
        if abs(Double(jw - gw)) / Double(gw) > 0.25 || abs(Double(jh - gh)) / Double(gh) > 0.25 { return nil }
    }
    return CGPoint(x: CGFloat(Double(jx) + rx * Double(jw)), y: CGFloat(Double(jy) + ry * Double(jh)))
}

/// Acha o alvo do passo (elemento ou ponto), esperando até `ms` por ele aparecer.
func esperarAlvo(_ p: [String: Any], pid: pid_t, ms: Int) -> (el: AXUIElement?, ponto: CGPoint?, como: String)? {
    let ax = p["ax"] as? [String: Any]
    let fim = Date().addingTimeInterval(Double(ms) / 1000)
    repeat {
        if execucao.pediuParar() { return nil }
        if let ax = ax, let (el, _) = acharElemento(alvo: ax, pid: pid), let c = centro(el) { return (el, c, "acessibilidade") }
        dormir(250)
    } while Date() < fim
    if let pt = pontoRelativo(p, pid: pid) { return (nil, pt, "coordenada") }
    return nil
}

/// Tela (monitor) onde está a janela focada do app — a mesma base da captura da exploração.
func telaDoApp(_ pid: pid_t) -> CGRect {
    if let j = janelaFocada(pid), let r = j.ret, let x = r["x"] as? Int, let y = r["y"] as? Int, let w = r["w"] as? Int, let h = r["h"] as? Int {
        var telas: [CGDirectDisplayID] = Array(repeating: 0, count: 8); var n: UInt32 = 0
        CGGetDisplaysWithRect(CGRect(x: x, y: y, width: w, height: h), 8, &telas, &n)
        if n > 0 { return CGDisplayBounds(telas[0]) }
    }
    return CGDisplayBounds(CGMainDisplayID())
}
/// v2: `x`/`y` em 0..1 relativos à tela inteira → ponto de tela.
func pontoDaTela(_ p: [String: Any], pid: pid_t) -> CGPoint? {
    guard let x = p["x"] as? Double, let y = p["y"] as? Double, x >= 0, x <= 1, y >= 0, y <= 1 else { return nil }
    let t = telaDoApp(pid)
    return CGPoint(x: t.minX + x * t.width, y: t.minY + y * t.height)
}
/// v2: item de menu do topo pelo nome (Arquivo › Exportar), via acessibilidade. nil = ok; texto = motivo.
func menuPorCaminho(_ caminho: [String], pid: pid_t) -> String? {
    guard (1...4).contains(caminho.count) else { return "menu: caminho inválido" }
    let axApp = AXUIElementCreateApplication(pid); AXUIElementSetMessagingTimeout(axApp, 1.0)
    guard let barra = axAtributo(axApp, kAXMenuBarAttribute) else { return "menu indisponível" }
    let barraEl = barra as! AXUIElement
    func filhos(_ e: AXUIElement) -> [AXUIElement] { axAtributo(e, kAXChildrenAttribute) as? [AXUIElement] ?? [] }
    func limpo(_ s: String) -> String { s.lowercased().replacingOccurrences(of: "…", with: "").replacingOccurrences(of: "...", with: "").trimmingCharacters(in: .whitespaces) }
    var itens = Array(filhos(barraEl).dropFirst()); var atual: AXUIElement? = nil
    for nome in caminho {
        let alvo = limpo(nome)
        guard let item = itens.first(where: { limpo(axTexto($0, kAXTitleAttribute)) == alvo }) ?? itens.first(where: { !alvo.isEmpty && limpo(axTexto($0, kAXTitleAttribute)).hasPrefix(alvo) }) else {
            return "menu \"\(nome)\" não encontrado"
        }
        let tit = limpo(axTexto(item, kAXTitleAttribute))
        if ["encerrar", "quit", "sair do", "desligar", "reiniciar", "log out", "terminar sessão", "forçar"].contains(where: { tit.hasPrefix($0) }) { return "menu bloqueado" }
        atual = item; itens = filhos(item).flatMap { filhos($0) }
    }
    guard let fim = atual else { return "menu: caminho inválido" }
    if let ativo = axAtributo(fim, kAXEnabledAttribute) as? Bool, !ativo { return "item de menu desativado" }
    return AXUIElementPerformAction(fim, kAXPressAction as CFString) == .success ? nil : "o menu não respondeu"
}

func executarRoteiro(_ roteiro: [String: Any], ignorar: String) {
    let passos = (roteiro["passos"] as? [[String: Any]]) ?? []
    let total = passos.count
    let inicio = agoraMs()
    if total == 0 { linha(["t": "erro", "motivo": "roteiro sem passos"]); return }
    var bundleAtual = ""
    // Roteiro v2 (escrito pelo agente no Explore e aprenda, 19/09): `bundle` no topo,
    // passos com `acao`/`t` = menu | clique{x,y da TELA} | esperar_janela | esperar |
    // tecla{mods} | ler. Passo desconhecido PARA (antes seguia calado).
    if let b = roteiro["bundle"] as? String, !b.isEmpty {
        if b == ignorar { linha(["t": "parou", "passo": 0, "de": total, "texto": "abrir app", "motivo": "o roteiro é dentro do próprio dn.os"]); return }
        if !ativarApp(bundle: b, nome: roteiro["app"] as? String ?? b) { linha(["t": "parou", "passo": 0, "de": total, "texto": "abrir app", "motivo": "não consegui abrir \(roteiro["app"] as? String ?? b)"]); return }
        bundleAtual = b
        dormir(400)
    }
    for (i, p) in passos.enumerated() {
        let n = i + 1
        let frase = fraseDoPasso(p)
        if execucao.pediuParar() { linha(["t": "parou", "passo": n, "de": total, "texto": frase, "motivo": "você parou"]); return }
        linha(["t": "passo", "n": n, "de": total, "texto": frase, "estado": "rodando"])
        let tipo = (p["t"] as? String) ?? (p["acao"] as? String) ?? ""
        // O app do passo: gravado no próprio passo (app.bundle); troca se preciso.
        if let a = p["app"] as? [String: Any], let b = a["bundle"] as? String, !b.isEmpty, b != bundleAtual {
            if b == ignorar { linha(["t": "parou", "passo": n, "de": total, "texto": frase, "motivo": "o passo é dentro do próprio dn.os"]); return }
            if !ativarApp(bundle: b, nome: a["nome"] as? String ?? "") { linha(["t": "parou", "passo": n, "de": total, "texto": frase, "motivo": "não consegui abrir \(a["nome"] as? String ?? b)"]); return }
            bundleAtual = b
            dormir(400)
        }
        guard let app = appDaFrente() else { linha(["t": "parou", "passo": n, "de": total, "texto": frase, "motivo": "nenhum app na frente"]); return }
        let pid = app.pid
        var falhou: String? = nil
        switch tipo {
        case "app":
            break
        case "clique", "duplo":
            if tipo == "clique", let ax = p["ax"] as? [String: Any], (ax["papel"] as? String) == "AXDockItem" {
                if !clicarNoDock(ax) { falhou = "não achei \"\(ax["titulo"] as? String ?? "o item")\" no Dock" }
            } else if p["ax"] == nil, p["coord"] == nil, let pt = pontoDaTela(p, pid: pid) {
                // v2: x/y 0..1 relativos à tela inteira (o que o agente viu na exploração).
                let direito = (p["botao"] as? String) == "direito"
                clicarEm(pt, direito: direito, duplo: tipo == "duplo" || (p["cliques"] as? Int) == 2)
            } else if let alvo = esperarAlvo(p, pid: pid, ms: 8000) {
                let direito = (p["botao"] as? String) == "direito"
                if tipo == "clique" && !direito, let el = alvo.el, ["AXButton", "AXMenuItem", "AXMenuBarItem", "AXCheckBox", "AXRadioButton", "AXPopUpButton", "AXLink", "AXDisclosureTriangle"].contains(axTexto(el, kAXRoleAttribute)), AXUIElementPerformAction(el, kAXPressAction as CFString) == .success {
                    // AXPress: o app executa a ação do controle sem depender da posição.
                } else if let pt = alvo.ponto {
                    clicarEm(pt, direito: direito, duplo: tipo == "duplo")
                } else { falhou = "alvo sem posição" }
            } else if !execucao.pediuParar() { falhou = "não achei o alvo na tela" }
        case "arrastar":
            let de = p["axDe"] as? [String: Any]
            var a: CGPoint? = nil
            if let de = de, let (el, _) = acharElemento(alvo: de, pid: pid) { a = centro(el) }
            if a == nil, let d = p["de"] as? [String: Any], let x = d["x"] as? Double, let y = d["y"] as? Double { a = CGPoint(x: x, y: y) }
            if let a = a, let alvo = esperarAlvo(p, pid: pid, ms: 6000), let b = alvo.ponto { arrastarDe(a, para: b) } else { falhou = "não achei de onde ou para onde arrastar" }
        case "digitar":
            let texto = p["valor"] as? String ?? ""
            if (p["senha"] as? Bool) == true { falhou = "senha: eu não digito; digite você e continue" }
            else {
                if let alvo = esperarAlvo(p, pid: pid, ms: 6000) {
                    var feito = false
                    if let el = alvo.el, ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"].contains(axTexto(el, kAXRoleAttribute)) {
                        if let pt = alvo.ponto { clicarEm(pt) }; dormir(120)
                        if AXUIElementSetAttributeValue(el, kAXValueAttribute as CFString, texto as CFTypeRef) == .success, norm(axTexto(el, kAXValueAttribute)) == norm(texto) { feito = true }
                    }
                    if !feito { if let pt = alvo.ponto { clicarEm(pt); dormir(120) }; digitarTexto(texto) }
                } else {
                    // Sem alvo: digita onde está o foco (o passo anterior já o colocou).
                    digitarTexto(texto)
                }
            }
        case "tecla":
            if let mods = p["mods"] as? [String], !mods.isEmpty {
                let combo = (mods + [p["tecla"] as? String ?? ""]).joined(separator: "+")
                if !atalhoPorNome(combo) { falhou = "atalho desconhecido: \(combo)" }
            } else if !teclaPorNome(p["tecla"] as? String ?? "") { falhou = "tecla desconhecida" }
        case "atalho":
            if !atalhoPorNome(p["tecla"] as? String ?? "") { falhou = "atalho desconhecido" }
        case "rolar":
            let pt = esperarAlvo(p, pid: pid, ms: 1500)?.ponto ?? pontoRelativo(p, pid: pid) ?? pontoDaTela(p, pid: pid) ?? CGPoint(x: 600, y: 400)
            let quanto = p["quanto"] as? Int ?? (p["dy"] as? Int).map { abs($0) } ?? 300
            let direcao = p["direcao"] as? String ?? ((p["dy"] as? Int ?? 0) < 0 ? "cima" : "baixo")
            rolarEm(pt, direcao: direcao, quanto: quanto)
        case "menu":
            if let r = menuPorCaminho(p["caminho"] as? [String] ?? [], pid: pid) { falhou = r }
        case "esperar_janela":
            let quer = (p["titulo_contem"] as? String ?? "").lowercased()
            let fim = Date().addingTimeInterval(Double(min(30000, p["ms"] as? Int ?? 8000)) / 1000)
            var achou = false
            repeat {
                if execucao.pediuParar() { break }
                let lista = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? [])
                if lista.contains(where: { ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) && (($0[kCGWindowName as String] as? String ?? "").lowercased().contains(quer)) }) { achou = true; break }
                dormir(250)
            } while Date() < fim
            if !achou { falhou = "a janela \"\(p["titulo_contem"] as? String ?? "")\" não apareceu" }
        case "esperar":
            dormir(min(10000, p["ms"] as? Int ?? 500))
        case "ler", "passar":
            if tipo == "passar", let pt = pontoDaTela(p, pid: pid) { mover(pt) }
        default:
            falhou = tipo.isEmpty ? "passo sem tipo" : "passo desconhecido: \(tipo)"
                }
        if let f = falhou {
            var out: [String: Any] = ["t": "parou", "passo": n, "de": total, "texto": frase, "motivo": f, "app": app.nome]
            if let j = janelaFocada(pid) { out["janela"] = j.titulo }
            if let foto = fotoDaJanela(pid) { out["foto"] = foto }
            linha(out); return
        }
        linha(["t": "passo", "n": n, "de": total, "texto": frase, "estado": "ok"])
        // Espera a tela assentar antes do próximo.
        dormir(tipo == "app" ? 300 : 450)
    }
    var fim: [String: Any] = ["t": "concluido", "ms": Int(agoraMs() - inicio), "de": total]
    if let a = appDaFrente() { fim["app"] = a.nome; if let j = janelaFocada(a.pid) { fim["janela"] = j.titulo }; if let foto = fotoDaJanela(a.pid) { fim["foto"] = foto } }
    linha(fim)
}

var ultimoEscMs: UInt64 = 0
func modoExecutar(caminho: String, ignorar: String) {
    guard let dados = FileManager.default.contents(atPath: caminho), let obj = try? JSONSerialization.jsonObject(with: dados), let roteiro = obj as? [String: Any] else {
        linha(["t": "erro", "motivo": "não li o roteiro em \(caminho)"]); exit(1)
    }
    if !acessibilidadeOk(pedir: false) { linha(["t": "erro", "motivo": "sem permissão de Acessibilidade: o dn.os não consegue clicar nem digitar"]); exit(2) }
    // Esc duas vezes (600 ms) para parar, de qualquer lugar. Tap só de escuta.
    let mascara: CGEventMask = (1 << CGEventType.keyDown.rawValue)
    let callback: CGEventTapCallBack = { _, tipo, ev, _ in
        if tipo == .keyDown, ev.getIntegerValueField(.keyboardEventKeycode) == 53 {
            let agora = agoraMs()
            if agora - ultimoEscMs < 600 { execucao.mandarParar() }
            ultimoEscMs = agora
        }
        return Unmanaged.passUnretained(ev)
    }
    if let tap = CGEvent.tapCreate(tap: .cghidEventTap, place: .headInsertEventTap, options: .listenOnly, eventsOfInterest: mascara, callback: callback, userInfo: nil) {
        CFRunLoopAddSource(CFRunLoopGetCurrent(), CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0), .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
    }
    DispatchQueue.global().async {
        while let l = readLine() { if l.trimmingCharacters(in: .whitespacesAndNewlines) == "parar" { execucao.mandarParar() } }
        execucao.mandarParar()
    }
    signal(SIGTERM) { _ in execucao.mandarParar() }
    linha(["t": "pronto", "hora": agoraMs(), "passos": (roteiro["passos"] as? [Any])?.count ?? 0])
    DispatchQueue.global(qos: .userInitiated).async {
        executarRoteiro(roteiro, ignorar: ignorar)
        exit(0)
    }
    CFRunLoopRun()
}

// ───────────────────────── main ─────────────────────────

let args = CommandLine.arguments.dropFirst()
let modo = args.first ?? "permissoes"
if modo == "permissoes" {
    let pedir = args.contains("--pedir")
    // Com --pedir, uma permissão por vez, na ordem: Acessibilidade primeiro (é a
    // obrigatória), depois Gravação de Tela. O macOS não mostra os dois diálogos
    // juntos — pedir os dois de uma vez deixava o segundo sem pergunta (13/09).
    var ax = acessibilidadeOk(pedir: false)
    var tela = telaOk(pedir: false)
    let mic = microfoneOk(pedir: false)
    if pedir {
        // Microfone NÃO se pede daqui (13/09): o macOS julga este binário pela
        // identidade dele e nega sem perguntar; a casca pede pelo próprio app.
        if !ax {
            ax = acessibilidadeOk(pedir: true)
        } else if !tela {
            tela = telaOk(pedir: true)
            if !tela {
                // Sequoia às vezes só registra o app na lista quando ele tenta capturar de fato.
                _ = CGWindowListCreateImage(CGRect(x: 0, y: 0, width: 1, height: 1), .optionOnScreenOnly, kCGNullWindowID, [])
                abrirPainel("Privacy_ScreenCapture")
            }
        }
    }
    linha(["acessibilidade": ax, "tela": tela, "microfone": mic])
    exit(0)
}
if modo == "gravar" {
    if let i = args.firstIndex(of: "--ignorar"), let b = args.dropFirst(i - args.startIndex + 1).first { gravador.ignorar = b }
    gravador.comFoto = !args.contains("--sem-foto")
    gravar()
}
if modo == "executar" {
    var caminho = "", ignorar = ""
    if let i = args.firstIndex(of: "--roteiro"), let c = args.dropFirst(i - args.startIndex + 1).first { caminho = c }
    if let i = args.firstIndex(of: "--ignorar"), let b = args.dropFirst(i - args.startIndex + 1).first { ignorar = b }
    modoExecutar(caminho: caminho, ignorar: ignorar)
}
linha(["t": "erro", "motivo": "modo desconhecido: \(modo)"])
exit(1)
