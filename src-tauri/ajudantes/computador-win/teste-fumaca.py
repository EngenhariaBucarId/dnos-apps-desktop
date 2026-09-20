#!/usr/bin/env python3
"""Teste de fumaça do ajudante do Windows, com o Bloco de Notas.

Roda no CI (windows-latest) e na sua máquina: abre o Bloco de Notas, observa,
digita, confere pela árvore de controles, executa um roteiro v2 e fecha.
Só imprime resumos (a foto vai em base64 e não cabe no log).

    python teste-fumaca.py [caminho\\do\\dnos-computador-win.exe]
"""
import json
import os
import subprocess
import sys
import tempfile
import time

AJUDANTE = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "target", "release", "dnos-computador-win.exe")
falhas = []


def rodar(args, entrada=None, timeout=30):
    r = subprocess.run([AJUDANTE, *args], input=entrada, capture_output=True, text=True, timeout=timeout)
    linhas = [l for l in r.stdout.splitlines() if l.strip()]
    if not linhas:
        return {"_sem_saida": True, "_stderr": r.stderr[-300:], "_codigo": r.returncode}
    return json.loads(linhas[-1]) if len(linhas) == 1 else [json.loads(l) for l in linhas]


def executar(arq, ignorar="ai.dnia.dnos", timeout=90):
    """Como a casca: stdin fica ABERTO durante o roteiro (fechar o stdin manda parar)."""
    proc = subprocess.Popen([AJUDANTE, "executar", "--roteiro", arq, "--ignorar", ignorar], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    eventos = []
    inicio = time.time()
    for l in proc.stdout:
        if l.strip():
            eventos.append(json.loads(l))
        if time.time() - inicio > timeout:
            proc.kill()
            break
    proc.stdin.close()
    proc.wait(timeout=10)
    return eventos


def resumo(o):
    o = dict(o)
    foto = o.pop("foto", None)
    if isinstance(foto, dict):
        o["foto"] = f'{foto.get("largura")}x{foto.get("altura")} ~{len(foto.get("dados", ""))//1024}KB'
    elif isinstance(foto, str):
        o["foto"] = f"~{len(foto)//1024}KB"
    ac = o.get("acessibilidade")
    if isinstance(ac, dict):
        o["acessibilidade"] = {"estado": ac.get("estado"), "controles": len(ac.get("controles", []))}
    return o


def conferir(nome, ok, detalhe=""):
    print(("OK   " if ok else "FALHA"), nome, detalhe)
    if not ok:
        falhas.append(nome)


def valores(o):
    return [c.get("valor", "") for c in o.get("acessibilidade", {}).get("controles", [])]


print("ajudante:", AJUDANTE)
p = rodar(["--permissoes"])
print("permissoes:", p)
conferir("permissoes", p.get("acessibilidade") is True and p.get("tela") is True and p.get("sistema") == "windows")

notepad = subprocess.Popen(["notepad.exe"])
time.sleep(4)
try:
    apps = rodar(["--apps"])
    nomes = [a["bundle"] for a in apps.get("apps", [])]
    print("apps:", nomes)
    conferir("apps lista o bloco de notas", "notepad.exe" in nomes)

    o = rodar(["--explorar", "notepad.exe"], timeout=40)
    print("explorar:", json.dumps(resumo(o), ensure_ascii=False)[:1500])
    conferir("explorar ok", o.get("ok") is True, str(o.get("motivo", "")))
    conferir("explorar traz foto", "foto" in o)
    conferir("explorar traz controles", len(o.get("acessibilidade", {}).get("controles", [])) > 0)
    conferir("explorar diz o sistema", o.get("sistema") == "windows")

    if o.get("ok"):
        base = {"bundle": "notepad.exe", "autonomo": True, "janela": o["janela"]["numero"], "captura": o["captura"]["ret"]}
        r = rodar(["--acao"], json.dumps({**base, "tipo": "texto", "descricao": "digitar", "texto": "ola dn.os fumaca"}))
        print("acao texto:", json.dumps(resumo(r), ensure_ascii=False)[:600])
        conferir("acao texto executada", r.get("executada") is True or r.get("ok") is True, str(r.get("motivo", "")))
        time.sleep(0.6)
        o2 = rodar(["--explorar", "notepad.exe"], timeout=40)
        digitou = any("ola dn.os fumaca" in v for v in valores(o2))
        conferir("o texto apareceu na árvore de controles", digitou)

        r = rodar(["--acao"], json.dumps({**base, "tipo": "tecla", "descricao": "selecionar tudo", "tecla": "a", "mods": ["cmd"]}))
        conferir("atalho cmd+a (vira ctrl+a)", r.get("executada") is True, str(r.get("motivo", "")))
        r = rodar(["--acao"], json.dumps({**base, "tipo": "tecla", "descricao": "fechar", "tecla": "f4", "mods": ["alt"]}))
        conferir("alt+f4 é bloqueado", r.get("motivo") == "atalho_bloqueado", str(r))
        r = rodar(["--acao"], json.dumps({**base, "tipo": "menu", "descricao": "menus", "caminho": [], "listar": True}))
        print("menu listar:", json.dumps(r, ensure_ascii=False)[:400])
        conferir("menu lista os menus do topo", isinstance(r.get("menu"), list) and len(r["menu"]) >= 3, str(r)[:200])
        # Um nível abaixo: abre o menu, lista e fecha (só informativo: a estrutura muda entre versões do Bloco de Notas).
        r = rodar(["--acao"], json.dumps({**base, "tipo": "menu", "descricao": "menu Format", "caminho": ["Format"], "listar": True}))
        print("menu Format (listar):", json.dumps(r, ensure_ascii=False)[:400])
        r = rodar(["--acao"], json.dumps({**base, "tipo": "menu", "descricao": "inexistente", "caminho": ["Nao existe"]}))
        conferir("menu inexistente diz as opções", "menu_nao_encontrado" in str(r.get("motivo", "")), str(r)[:200])
        r = rodar(["--acao"], json.dumps({**base, "tipo": "clique", "descricao": "fora do app", "x": 0.001, "y": 0.001}))
        print("clique no canto da tela:", json.dumps(r, ensure_ascii=False)[:200])

    # Roteiro v2 pelo executor (o mesmo caminho da casca).
    jr, cr = o["janela"]["ret"], o["captura"]["ret"]
    cx, cy = (jr["x"] + jr["w"] * 0.5 - cr["x"]) / cr["w"], (jr["y"] + jr["h"] * 0.5 - cr["y"]) / cr["h"]
    roteiro = {
        "nome": "fumaca", "bundle": "notepad.exe", "app": "Bloco de Notas", "criterio_sucesso": "texto no bloco",
        "passos": [
            {"acao": "esperar_janela", "titulo_contem": "notepad", "ms": 3000, "descricao": "esperar a janela"},
            {"acao": "clique", "x": round(cx, 3), "y": round(cy, 3), "descricao": "clicar no meio da janela"},
            {"acao": "esperar", "ms": 300, "descricao": "esperar"},
            {"acao": "tecla", "tecla": "a", "mods": ["cmd"], "descricao": "selecionar tudo"},
            {"acao": "digitar", "valor": "roteiro rodou", "descricao": "digitar"},
            {"acao": "ler", "descricao": "conferir"},
        ],
    }
    arq = os.path.join(tempfile.gettempdir(), "roteiro-fumaca.json")
    with open(arq, "w", encoding="utf-8") as f:
        json.dump(roteiro, f)
    eventos = executar(arq)
    print("roteiro:", json.dumps([resumo(e) for e in eventos], ensure_ascii=False)[:1500])
    conferir("roteiro concluiu", any(e.get("t") == "concluido" for e in eventos), str([e for e in eventos if e.get("t") == "parou"]))
    o3 = rodar(["--explorar", "notepad.exe"], timeout=40)
    conferir("roteiro digitou", any("roteiro rodou" in v for v in valores(o3)))

    # Passo desconhecido tem de PARAR, não seguir calado.
    with open(arq, "w", encoding="utf-8") as f:
        json.dump({"nome": "x", "bundle": "notepad.exe", "passos": [{"acao": "voar", "descricao": "passo inventado"}]}, f)
    eventos = executar(arq)
    conferir("passo desconhecido para", any(e.get("t") == "parou" and "desconhecido" in e.get("motivo", "") for e in eventos), str(eventos)[:300])
finally:
    notepad.kill()
    subprocess.run(["taskkill", "/F", "/IM", "notepad.exe"], capture_output=True)

print()
print("RESULTADO:", "tudo certo" if not falhas else f"{len(falhas)} falha(s): {falhas}")
sys.exit(1 if falhas else 0)
