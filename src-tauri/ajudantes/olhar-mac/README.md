# Olhar — protótipo 2C-1

Captura pontual e somente de leitura, independente do gravador em produção.
Ainda não está embutido no dn.os Desktop e não está ligado ao socket.

Compilar: `bash src-tauri/ajudantes/olhar-mac/compilar.sh`.
O binário universal sai em `/tmp/dnos-olhar-mac-universal`; um argumento ao script
permite escolher outro destino. Use um destino fora do repositório.

- `--permissoes`: estado de Acessibilidade e Gravação de Tela, sem abrir diálogos.
- sem argumentos: janela visível do aplicativo da frente.
- `--app com.lemon.lvoverseas`: janela visível do CapCut já aberto.
- `--app com.apple.finder`: janela visível do Finder já aberto.

Saída: uma linha JSON, com `ok`, `app`, `janela`, `acessibilidade`, `foto`,
`avisos`, `consistente`, `capturado_em`, `duracao_ms`, `somente_leitura`.
`foto.dados` é JPEG em base64; `foto.ret` guarda os limites originais da janela.
A árvore é limitada a 150 controles, profundidade 8, fila 300 e orçamento de
2 segundos (cada chamada AX também tem timeout). O prazo externo do processo
precisa ser imposto pelo chamador: um app travado pode atrasar APIs do sistema.

A árvore só é usada quando corresponde à janela fotografada. Campos AX seguros
não exportam texto; encontrar um campo protegido omite a foto inteira. Apps sem
AX não permitem garantir identificação de campos sensíveis desenhados na tela.

O teste de 18/09 compilou para arm64/x86_64. O processo de teste não tinha
permissão AX/tela; os dois casos retornaram `permissoes_necessarias`. Não houve
captura ou validação visual. Validar dentro do app assinado antes da liberação.

Integração planejada: o host embute o binário, conserva a sessão/requisição do
relay, exige consentimento local, controla prazo/Parar/concorrência, e envia uma
`olhar-resposta`. Não encaminhar foto para logs ou eventos globais da webview.
