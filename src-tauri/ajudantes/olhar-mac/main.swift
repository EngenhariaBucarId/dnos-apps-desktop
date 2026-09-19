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
// 19/09: lista suspensa, menu e dica do app são janelas próprias (outra camada)
// por cima da lida; capturar só a janela deixava o agente sem ver a lista de
// resoluções do Exportar do CapCut. Compõe TODAS as janelas do app visíveis
// sobre o retângulo da janela lida (só do app: nada de outros programas).
func capturarComPopups(_ numero: UInt32, _ ret: CGRect) -> CGImage? {
    let todas = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? [])
        .filter { ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) && (($0[kCGWindowLayer as String] as? Int) ?? 0) >= 0 }
        .filter { w in (w[kCGWindowBounds as String] as? [String: Any]).flatMap { CGRect(dictionaryRepresentation: $0 as CFDictionary) }.map { $0.intersects(ret) } ?? false }
        .compactMap { $0[kCGWindowNumber as String] as? UInt32 }
    let ids = todas.contains(numero) ? todas : [numero] + todas
    let ptr = UnsafeMutablePointer<UnsafeRawPointer?>.allocate(capacity: ids.count)
    defer { ptr.deallocate() }
    for (i, id) in ids.enumerated() { ptr[i] = UnsafeRawPointer(bitPattern: UInt(id)) }
    if let arr = CFArrayCreate(kCFAllocatorDefault, ptr, ids.count, nil),
       let img = CGImage(windowListFromArrayScreenBounds: ret, windowArray: arr, imageOption: [.boundsIgnoreFraming, .nominalResolution]),
       img.width > 1 { return img }
    return CGWindowListCreateImage(.null, .optionIncludingWindow, CGWindowID(numero), [.boundsIgnoreFraming, .nominalResolution])
}
// Seleciona uma única janela. A foto e a árvore devem descrever a MESMA janela.
let janelas = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? [])
    .filter { ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) && ($0[kCGWindowLayer as String] as? Int) == 0 }
// Qual janela do app ler (18–19/09):
// 1) a janela com foco, se não for painel pequeno — é onde a pessoa/agente
//    está agindo; o diálogo de Exportar do CapCut é uma janela separada e com
//    foco, e ler só a principal deixava o agente cego para ele;
// 2) a janela grande (≥300×200) mais à frente — o painel flutuante "EditPilot"
//    de 183×88 do CapCut fica por cima e não conta; 3) a principal; 4) a da frente.
func limitesDe(_ w: [String: Any]) -> CGRect? {
    (w[kCGWindowBounds as String] as? [String: Any]).flatMap { CGRect(dictionaryRepresentation: $0 as CFDictionary) }
}
func grande(_ r: CGRect) -> Bool { r.width >= 300 && r.height >= 200 }
func mesma(_ a: CGRect, _ b: CGRect) -> Bool { abs(a.minX - b.minX) < 2 && abs(a.minY - b.minY) < 2 && abs(a.width - b.width) < 2 && abs(a.height - b.height) < 2 }
let axDoApp: AXUIElement? = axOK ? {
    let a = AXUIElementCreateApplication(pid); AXUIElementSetMessagingTimeout(a, 0.15); return a
}() : nil
func janelaAX(_ atr: String) -> [String: Any]? {
    guard let a = axDoApp, let r = atributo(a, atr).flatMap({ retangulo($0 as! AXUIElement) }) else { return nil }
    return janelas.first { w in limitesDe(w).map { mesma($0, r) } ?? false }
}
func escolherJanela() -> [String: Any]? {
    if let f = janelaAX(kAXFocusedWindowAttribute), let r = limitesDe(f), grande(r) { return f }
    // A lista vem da frente para trás: a primeira grande é o diálogo, se houver.
    if let frente = janelas.first(where: { limitesDe($0).map(grande) ?? false }) { return frente }
    return janelaAX(kAXMainWindowAttribute) ?? janelas.first
}
let janelaEscolhida: [String: Any]? = escolherJanela()
// O agente precisa saber que há outras janelas (diálogo, painel) além da lida.
saida["janelas_do_app"] = janelas.prefix(8).map { w -> [String: Any] in
    let r = limitesDe(w) ?? .zero
    return ["titulo": w[kCGWindowName as String] as? String ?? "", "w": Int(r.width), "h": Int(r.height),
            "lida": (w[kCGWindowNumber as String] as? UInt32) == (janelaEscolhida?[kCGWindowNumber as String] as? UInt32)]
}
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
        let limite = Date().addingTimeInterval(pedido == nil ? 2 : 0.7)
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
else if let img = capturarComPopups(numero, ret) {
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
    // Autônomo (19/09): a janela precisa ser a mesma e sem campo de senha, mas
    // não pixel a pixel — num editor de vídeo a tela muda sozinha o tempo todo.
    // Acompanhado continua exigindo a impressão idêntica à observação aprovada.
    let autonomo = pedido["autonomo"] as? Bool == true
    let impressaoConfere = (pedido["impressao"] as? String).map { saida["impressao"] as? String == $0 } ?? false
    guard !mudou, !senha, pedido["janela"] as? UInt32 == numero, autonomo || impressaoConfere else {
        resposta(["ok":false,"motivo":"tela_mudou_observe_novamente"])
    }
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
    func mouse(_ tipo: CGEventType,_ pt: CGPoint, botao: CGMouseButton = .left, contagem: Int64 = 1) {
        guard let ev = CGEvent(mouseEventSource:nil,mouseType:tipo,mouseCursorPosition:pt,mouseButton:botao) else { return }
        if tipo != .mouseMoved && tipo != .leftMouseDragged { ev.setIntegerValueField(.mouseEventClickState, value: contagem) }
        ev.post(tap:.cghidEventTap)
    }
    func esperar(_ s: Double) { Thread.sleep(forTimeInterval: s) }
    let codigos: [String: CGKeyCode] = [
        "a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,"q":12,"w":13,"e":14,"r":15,"y":16,"t":17,
        "1":18,"2":19,"3":20,"4":21,"6":22,"5":23,"=":24,"9":25,"7":26,"-":27,"8":28,"0":29,"]":30,"o":31,"u":32,"[":33,
        "i":34,"p":35,"l":37,"j":38,"'":39,"k":40,";":41,"\\":42,",":43,"/":44,"n":45,"m":46,".":47,"`":50,
        "enter":36,"tab":48,"espaco":49,"backspace":51,"escape":53,"delete":117,"home":115,"end":119,"pageup":116,"pagedown":121,
        "esquerda":123,"direita":124,"baixo":125,"cima":126,
        "down":125,"up":126,"left":123,"right":124,"return":36,"space":49,"esc":53,"pgup":116,"pgdown":121,
        "f1":122,"f2":120,"f3":99,"f4":118,"f5":96,"f6":97,"f7":98,"f8":100,"f9":101,"f10":109,"f11":103,"f12":111]
    if t == "clique" || t == "passar" {
        guard let pt=ponto("x","y"),conferir(pt) else { resposta(["ok":false,"motivo":"alvo_fora_do_app_ou_protegido"]) }
        mouse(.mouseMoved,pt); esperar(0.06)
        if t == "clique" {
            let direito = pedido["botao"] as? String == "direito"
            let (baixo, cima, botao): (CGEventType, CGEventType, CGMouseButton) = direito ? (.rightMouseDown,.rightMouseUp,.right) : (.leftMouseDown,.leftMouseUp,.left)
            let vezes = (pedido["cliques"] as? Int) == 2 ? 2 : 1
            for n in 1...vezes {
                mouse(baixo,pt,botao:botao,contagem:Int64(n)); esperar(0.04)
                mouse(cima,pt,botao:botao,contagem:Int64(n)); if vezes == 2 { esperar(0.09) }
            }
        }
    } else if t == "arrastar" {
        guard let pt=ponto("x","y"),conferir(pt) else { resposta(["ok":false,"motivo":"alvo_fora_do_app_ou_protegido"]) }
        guard let f=ponto("x2","y2"),conferir(f) else { resposta(["ok":false,"motivo":"destino_fora_do_app"]) }
        mouse(.mouseMoved,pt); esperar(0.06)
        mouse(.leftMouseDown,pt); esperar(0.08)
        for i in 1...20 { mouse(.leftMouseDragged,CGPoint(x:pt.x+(f.x-pt.x)*Double(i)/20,y:pt.y+(f.y-pt.y)*Double(i)/20)); esperar(0.015) }
        esperar(0.05); mouse(.leftMouseUp,f)
    } else if t == "rolar" {
        guard let pt=ponto("x","y"),conferir(pt) else { resposta(["ok":false,"motivo":"rolagem_invalida"]) }
        let dy = pedido["dy"] as? Int ?? 0, dx = pedido["dx"] as? Int ?? 0
        guard abs(dy)<=600, abs(dx)<=600, dx != 0 || dy != 0 else { resposta(["ok":false,"motivo":"rolagem_invalida"]) }
        mouse(.mouseMoved,pt); esperar(0.03)
        let ev=CGEvent(scrollWheelEvent2Source:nil,units:.pixel,wheelCount:2,wheel1:Int32(dy),wheel2:Int32(dx),wheel3:0); ev?.location=pt;ev?.post(tap:.cghidEventTap)
    } else if t == "texto" || t == "tecla" {
        let axApp=AXUIElementCreateApplication(pid)
        // Sem foco (timeline, canvas) o atalho vai para o app, que já está na
        // frente; só não digita nem aperta tecla com campo de senha em foco.
        if let foco=elemento(atributo(axApp,kAXFocusedUIElementAttribute)),
           texto(foco,kAXRoleAttribute) == "AXSecureTextField" || texto(foco,kAXSubroleAttribute) == "AXSecureTextField" {
            resposta(["ok":false,"motivo":"foco_protegido"])
        }
        if t == "texto" {
            guard let valor=pedido["texto"] as? String,valor.utf16.count<=1000,!valor.contains("\n"),!valor.contains("\r") else { resposta(["ok":false,"motivo":"texto_invalido"]) }
            let chars=Array(valor.utf16)
            for baixo in [true,false] { let ev=CGEvent(keyboardEventSource:nil,virtualKey:0,keyDown:baixo);ev?.keyboardSetUnicodeString(stringLength:chars.count,unicodeString:chars);ev?.post(tap:.cghidEventTap) }
        } else {
            guard let nome=(pedido["tecla"] as? String)?.lowercased(),let tecla=codigos[nome] else { resposta(["ok":false,"motivo":"tecla_nao_permitida"]) }
            let mods = Set(pedido["mods"] as? [String] ?? [])
            // Nunca: sair do app, trocar de app, Spotlight, forçar encerrar, sair da conta.
            let bloqueado = mods.contains("cmd") && (["q","tab","espaco","h"].contains(nome) || (mods.contains("alt") && nome == "escape") || (mods.contains("shift") && nome == "q"))
            guard !bloqueado else { resposta(["ok":false,"motivo":"atalho_bloqueado"]) }
            var flags = CGEventFlags()
            if mods.contains("cmd") { flags.insert(.maskCommand) }
            if mods.contains("shift") { flags.insert(.maskShift) }
            if mods.contains("alt") { flags.insert(.maskAlternate) }
            if mods.contains("ctrl") { flags.insert(.maskControl) }
            for baixo in [true,false] {
                let ev=CGEvent(keyboardEventSource:nil,virtualKey:tecla,keyDown:baixo); ev?.flags=flags; ev?.post(tap:.cghidEventTap)
                if baixo { esperar(0.03) }
            }
        }
    } else if t == "menu" {
        // Menus do topo pelo próprio app (acessibilidade), sem coordenada: a barra
        // de menus fica fora da janela. Sem o menu da Apple, nunca "Encerrar".
        // caminho vazio + listar = os menus do topo (Arquivo, Editar, …).
        let listar = pedido["listar"] as? Bool == true
        guard let caminho = pedido["caminho"] as? [String], (listar ? 0 : 1)...4 ~= caminho.count else { resposta(["ok":false,"motivo":"menu_invalido"]) }
        let axApp=AXUIElementCreateApplication(pid); AXUIElementSetMessagingTimeout(axApp, 1.0)
        guard let barra=elemento(atributo(axApp,kAXMenuBarAttribute)) else { resposta(["ok":false,"motivo":"menu_indisponivel"]) }
        func filhos(_ e: AXUIElement) -> [AXUIElement] { atributo(e,kAXChildrenAttribute) as? [AXUIElement] ?? [] }
        func limpo(_ s: String) -> String { s.lowercased().replacingOccurrences(of:"…",with:"").replacingOccurrences(of:"...",with:"").trimmingCharacters(in:.whitespaces) }
        var itens = Array(filhos(barra).dropFirst())
        var atual: AXUIElement? = nil
        for nome in caminho {
            let alvo = limpo(nome)
            guard let item = itens.first(where:{limpo(texto($0,kAXTitleAttribute)) == alvo}) ?? itens.first(where:{ !alvo.isEmpty && limpo(texto($0,kAXTitleAttribute)).hasPrefix(alvo) }) else {
                let opcoes = itens.map{texto($0,kAXTitleAttribute)}.filter{!$0.isEmpty}.prefix(40).joined(separator:" | ")
                resposta(["ok":false,"motivo":"menu_nao_encontrado:\(nome) · opções: \(opcoes)"])
            }
            let tit = limpo(texto(item,kAXTitleAttribute))
            if ["encerrar","quit","sair do","desligar","reiniciar","log out","terminar sessão","forçar"].contains(where:{tit.hasPrefix($0)}) { resposta(["ok":false,"motivo":"menu_bloqueado"]) }
            atual = item
            itens = filhos(item).flatMap { filhos($0) }
        }
        if listar && caminho.isEmpty {
            resposta(["ok":true,"executada":false,"menu":itens.map{texto($0,kAXTitleAttribute)}.filter{!$0.isEmpty}.prefix(60).map{$0}])
        }
        guard let final = atual else { resposta(["ok":false,"motivo":"menu_invalido"]) }
        if listar {
            resposta(["ok":true,"executada":false,"menu":itens.map{texto($0,kAXTitleAttribute)}.filter{!$0.isEmpty}.prefix(60).map{$0}])
        }
        if let ativo = atributo(final,kAXEnabledAttribute) as? Bool, !ativo { resposta(["ok":false,"motivo":"menu_desativado"]) }
        guard AXUIElementPerformAction(final,kAXPressAction as CFString) == .success else { resposta(["ok":false,"motivo":"menu_nao_respondeu"]) }
    } else { resposta(["ok":false,"motivo":"acao_nao_permitida"]) }
    resposta(["ok":true,"executada":true])
}
resposta(saida)
