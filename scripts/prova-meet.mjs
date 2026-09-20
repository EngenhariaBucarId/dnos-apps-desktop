// Prova do roteiro do Meet contra uma sala REAL (20/09/2026).
//
// O roteiro que leva o agente para dentro do Meet (`ROTEIRO` e `IDIOMA` em
// src-tauri/src/reuniao_meet.rs) depende dos rótulos que o Google desenha na
// tela — e o Google muda. Este script roda EXATAMENTE esses dois textos contra
// uma aba aberta do Meet, num laço parecido com o da casca, e imprime as
// transições de estado. Foi ele que achou, no primeiro dia, dois defeitos que
// o compilador não pega: o agente ficava "entrando" para sempre depois que o
// Meet voltava para a tela inicial, e o idioma tentava cedo demais.
//
// Como usar:
//   1. Abra o Chrome do agente com a porta de depuração e uma sala:
//        open -na "Google Chrome" --args \
//          --user-data-dir="$HOME/Library/Application Support/ai.dnia.dnos/chrome-agentes/milo" \
//          --remote-debugging-port=19340 --mute-audio https://meet.google.com/new
//      (meet.new cria uma sala do próprio agente: ninguém mais entra nela.)
//   2. node scripts/prova-meet.mjs 15     # segundos de laço
//   3. Feche o Chrome do agente quando terminar.
// Termine sempre saindo da sala e fechando a porta 19340.
// Simula o laço da casca com os MESMOS scripts do reuniao_meet.rs.
import { readFileSync } from "node:fs";
const rs = readFileSync(new URL("../src-tauri/src/reuniao_meet.rs", import.meta.url), "utf8");
const pega = (nome) => { const m = rs.match(new RegExp(`const ${nome}: &str = r#"([\\s\\S]*?)"#;`)); if (!m) throw new Error("sem " + nome); return m[1]; };
const ROTEIRO = pega("ROTEIRO"), IDIOMA = pega("IDIOMA");
const segundos = Number(process.argv[2] || 20);
const lista = await (await fetch("http://127.0.0.1:19340/json/list")).json();
const aba = lista.find((t) => t.type === "page" && t.url.includes("meet.google.com"));
const ws = new WebSocket(aba.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r));
let id = 0; const pend = new Map();
ws.addEventListener("message", (e) => { const m = JSON.parse(e.data); if (pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
const avalia = (expression, extra = {}) => new Promise((res) => { const i = ++id; pend.set(i, res); ws.send(JSON.stringify({ id: i, method: "Runtime.evaluate", params: { expression, returnByValue: true, userGesture: true, ...extra } })); });
let ultimo = "", idiomaOk = false, tent = 0, ultTent = 0;
const t0 = Date.now();
while ((Date.now() - t0) / 1000 < segundos) {
  const r = await avalia(ROTEIRO);
  const v = JSON.parse(r.result.result.value);
  const chave = v.estado + (v.pessoas != null ? ` pessoas=${v.pessoas}` : "");
  if (chave !== ultimo) { console.log(`[${((Date.now() - t0) / 1000).toFixed(1)}s]`, chave, v.semPainel ? "(sem painel de legendas)" : ""); ultimo = chave; }
  if (v.estado === "encerrada" || v.estado === "negado") { console.log("→ a casca sairia com motivo:", v.estado); break; }
  if (v.estado === "na-reuniao" && v.legendasOn && !idiomaOk && tent < 6 && Date.now() - ultTent > 4000) {
    tent++; ultTent = Date.now();
    const l = await avalia(IDIOMA, { awaitPromise: true });
    console.log("   idioma (tentativa", tent + "):", JSON.stringify(l.result.result.value));
    idiomaOk = l.result.result.value?.ok === true;
  }
  await new Promise((r) => setTimeout(r, 500));
}
ws.close();
