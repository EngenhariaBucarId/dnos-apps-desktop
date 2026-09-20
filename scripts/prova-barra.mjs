// Prova da barra flutuante (20/09/2026).
//
// A barra serve a seis modos e o botão Parar chama uma operação diferente em
// cada um. Errar isso é grave: o Parar de uma reunião não pode encerrar uma
// execução, e a barra do Meet não pode acender o microfone (lá quem ouve é o
// Meet, nenhum microfone desta máquina está aberto).
//
// Este script desenha a barra de verdade, fora do app, e mostra o que cada modo
// produz: classe, título, subtítulo, botão e o evento que o Parar emite.
//
// Como usar (o jsdom vive no repositório da web):
//   cd ../missioncontroldnia && node ../dnos-apps-desktop/scripts/prova-barra.mjs
// O processo fica vivo por causa do relógio interno da barra: encerre com Ctrl+C.
import { readFileSync } from "node:fs";
import { JSDOM } from "jsdom";
const html = readFileSync(new URL("../ui/barra-mac.html", import.meta.url), "utf8");
const dom = new JSDOM(html, { runScripts: "dangerously" });
const w = dom.window;
let ouvinte = null; const emitidos = [];
w.__TAURI__ = {
  event: { listen: async (n, cb) => { if (n === "dnos://barra-mac") ouvinte = cb; return () => {}; }, emit: async (n, p) => { emitidos.push(n); } },
  core: { invoke: async (n, a) => { emitidos.push(`${n}:${a?.acao ?? ""}`); } },
};
await new Promise((r) => setTimeout(r, 60));
const $ = (id) => w.document.getElementById(id);
for (const [modo, sub] of [["reuniao", "o áudio não é guardado — só a transcrição"], ["meet", "ouvindo pelas legendas — sem microfone, sem câmera"], ["assistindo", "3 passos"]]) {
  emitidos.length = 0;
  await ouvinte({ payload: { modo, agente: "Milo", sub, ouvindo: modo === "reuniao" } });
  $("parar").onclick();
  await new Promise((r) => setTimeout(r, 10));
  console.log(`\n[${modo}]`);
  console.log("  classe :", $("barra").className);
  console.log("  título :", $("titulo").textContent.trim());
  console.log("  sub    :", $("sub").textContent.trim());
  console.log("  botão  :", JSON.stringify($("parar").textContent), "|", $("parar").title);
  console.log("  micro  :", $("mic").className);
  console.log("  Parar →", emitidos.join(", "));
}
