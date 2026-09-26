//! Sessão de aprendizado: um agente, aplicativo e rodada escolhidos pela pessoa.
//! Cada ação consome uma observação. Nunca aceita shell, scripts ou caminhos.
use serde_json::{json,Value};
use std::{io::{Read,Write},process::{Command,Stdio},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
use tauri::{AppHandle,Manager};
use tokio::sync::mpsc::UnboundedSender;
use crate::maquina::{self,ReservaDeUso};
#[cfg(target_os="macos")] const AJUDANTE:&[u8]=include_bytes!("../ajudantes/dnos-olhar-mac");
// Windows (0.8.0): o mesmo contrato, escrito em Rust (ajudantes/computador-win). O CI o compila antes da casca.
#[cfg(windows)] const AJUDANTE:&[u8]=include_bytes!("../ajudantes/dnos-computador-win.exe");
#[cfg(not(any(target_os="macos",windows)))] const AJUDANTE:&[u8]=&[];
use crate::maquina::sem_console;
struct Sessao { id:String,agente:String,nome:String,app_nome:String,bundle:String,ate:u64,autonomia:bool,acoes:u32,leituras:u32,iniciada_em:u64,fase:String,
 cancelada:Arc<AtomicBool>,ocupada:bool,ultima:Option<(String,u64,Value)>,pendente:Option<Value>,resultados:Vec<(String,Value)>,_reserva:ReservaDeUso,
 // 0.8.5 (modo livre, fase 2): o chat acompanha a sessão pelo próprio app. `tipo`
 // separa tarefa de aprendizado; `passos` são as últimas ações (descrição e
 // resultado); `foto` é a última captura, entregue só à janela principal.
 tipo:String,passos:Vec<Value>,foto:Option<(u64,Value)> }
const MAX_PASSOS:usize=40;
fn registrar_passo(passos:&mut Vec<Value>,v:Value){passos.push(v);if passos.len()>MAX_PASSOS{passos.remove(0);}}
#[derive(Default)] pub struct Estado { conexao:Option<(u64,UnboundedSender<String>)>,sessao:Option<Sessao>,ultimo_motivo:Option<String> }
type Compartilhado=Arc<Mutex<Estado>>;
fn agora()->u64 {crate::gravador::agora_ms() as u64}
fn uuid(s:&str)->bool {s.len()==36 && s.bytes().all(|c|c.is_ascii_hexdigit()||c==b'-')}
fn app_permitido(s:&str)->bool {
 // `_` e espaço existem em nomes de executáveis do Windows ("Adobe Premiere Pro.exe").
 let baixo=s.to_lowercase();
 !s.is_empty() && s.len()<200 && s.bytes().all(|c|c.is_ascii_alphanumeric()||b".-_ ".contains(&c))
 && !["terminal","iterm","keychain","systempreferences","password","1password","dnos","powershell","pwsh","windowsterminal","conhost","regedit","taskmgr","systemsettings","keepass","bitwarden","lastpass","dashlane","enpass","nordpass","mintty"].iter().any(|x|baixo.contains(x))
 && !["cmd.exe","wt.exe","wsl.exe","bash.exe","mmc.exe","control.exe"].contains(&baixo.as_str())
}
fn enviar(g:&Estado,v:Value) {if let Some((_,tx))=&g.conexao {let _=tx.send(v.to_string());}}
fn encerrar(app:&AppHandle,g:&mut Estado,motivo:&str) {
 if let Some(s)=g.sessao.take(){g.ultimo_motivo=Some(motivo.to_owned());s.cancelada.store(true,Ordering::SeqCst);enviar(g,json!({"t":"explorar-fim","sessao":s.id,"motivo":motivo}));enviar(g,json!({"t":"explorar-permissao","permitido":false}));maquina::esconder_barra(app);}
}
fn estado(g:&Estado)->Value {match &g.sessao {
 Some(s)=>json!({"disponivel":!AJUDANTE.is_empty(),"conectado":g.conexao.is_some(),"ativo":true,"id":s.id,"agente":s.agente,"bundle":s.bundle,"expira_em":s.ate,"acoes":s.acoes,"leituras":s.leituras,"fase":s.fase,"pendente":s.pendente,"tipo":s.tipo,"nome":s.nome,"app_nome":s.app_nome,"autonomia":s.autonomia,"passos":s.passos,"tem_tela":s.foto.is_some()}),
 None=>json!({"disponivel":!AJUDANTE.is_empty(),"conectado":g.conexao.is_some(),"ativo":false,"ultimo_motivo":g.ultimo_motivo})}}
fn rotulo_fase(fase:&str)->&str {match fase {
 "aguardando"=>"Aguardando o agente iniciar", "demorado"=>"O agente ainda não iniciou; confira o chat",
 "observando"=>"Capturando a janela", "aguardando_leitura"=>"Captura pronta; aguardando leitura",
 "interpretando"=>"Interpretando a imagem", "agindo"=>"Executando uma ação", "decidindo"=>"Analisando o próximo passo",
 _=>"Sessão autorizada"}}
fn barra(app:&AppHandle,s:&Sessao) {
 let sub=if let Some(p)=&s.pendente {format!("Aprovar: {}",p["passo"]["descricao"].as_str().unwrap_or("próxima ação"))} else {format!("{} · {} ações (limite 300)",rotulo_fase(&s.fase),s.acoes)};
 maquina::falar_na_barra(app,if s.pendente.is_some(){"explorar-aprovacao"}else{"explorando"},&s.nome,&s.app_nome,sub,false);
}
#[tauri::command]
pub async fn explorar_computador(app:AppHandle,window:tauri::WebviewWindow,acao:String,pedido:Option<Value>)->Result<Value,String>{
 let barra_local=window.label()=="barra-mac" && ["estado","parar","aprovar","recusar"].contains(&acao.as_str());
 if !barra_local && window.label()!="main" {return Err("janela_nao_autorizada".into());}
 if !barra_local {
  let url=window.url().map_err(|_|"origem_indisponivel")?;
  if url.origin()!=url::Url::parse(&crate::url_da_instancia()).map_err(|_|"instancia_invalida")?.origin(){return Err("origem_nao_autorizada".into());}
 }
 if acao=="autorizar" {
  let h=app.clone();
  let permissoes=tauri::async_runtime::spawn_blocking(move||ajudante(&h,&["--permissoes"],None,&AtomicBool::new(false))).await.map_err(|_|"consulta_falhou")??;
  if permissoes["acessibilidade"]!=true||permissoes["tela"]!=true{return Err("Autorize Acessibilidade e Gravação de Tela para o dn.os antes de iniciar a exploração.".into());}
 }
 if acao=="apps" {let h=app.clone();return tauri::async_runtime::spawn_blocking(move||ajudante(&h,&["--apps"],None,&AtomicBool::new(false))).await.map_err(|_|"consulta_falhou")?;}
 let e=app.state::<Compartilhado>();let mut g=e.lock().map_err(|_|"sessao_indisponivel")?;
 if g.sessao.as_ref().map(|s|s.ate<=agora()).unwrap_or(false){encerrar(&app,&mut g,"tempo_esgotado");}
 let p=pedido.unwrap_or(json!({}));
 match acao.as_str(){
 "estado"=>{},"parar"=>encerrar(&app,&mut g,"pessoa"),
 // A tela atual vai direto do app para o chat, sem passar pelo servidor.
 "tela"=>{let s=g.sessao.as_ref().ok_or("sessao_encerrada")?;
  return Ok(match &s.foto {Some((hora,f))=>json!({"ok":true,"hora":hora,"mime":f["mime"],"dados":f["dados"],"largura":f["largura"],"altura":f["altura"]}),None=>json!({"ok":true,"vazia":true})});},
 "autorizar"=>{
  if AJUDANTE.is_empty()||g.conexao.is_none(){return Err("Abra o Desktop atualizado e espere a conexão.".into());}
  if g.sessao.is_some(){return Err("Encerre a sessão anterior.".into());}
  let id=p["id"].as_str().filter(|s|uuid(s)).ok_or("rodada_obrigatoria")?.to_owned();
  let agente=p["agente"].as_str().filter(|s|!s.is_empty()&&s.len()<=100).ok_or("agente_obrigatorio")?.to_owned();
  let nome=p["nome"].as_str().unwrap_or(&agente).chars().take(100).collect::<String>();
  let bundle=p["bundle"].as_str().filter(|s|app_permitido(s)).ok_or("aplicativo_nao_permitido")?.to_owned();
  let app_nome=p["app_nome"].as_str().unwrap_or(&bundle).chars().take(100).collect::<String>();
  let minutos=p["minutos"].as_u64().filter(|m|[5,10,15,30,60].contains(m)).ok_or("prazo_invalido")?;
  let autonomia=p["autonomia"].as_bool().unwrap_or(false);let ate=agora()+minutos*60_000;
  let tipo=if p["tipo"]=="tarefa"{"tarefa"}else{"aprendizado"}.to_owned();
  let reserva=maquina::reservar_uso(&app,"explorando")?;
  g.ultimo_motivo=None;g.sessao=Some(Sessao{id:id.clone(),agente:agente.clone(),nome,app_nome,bundle:bundle.clone(),ate,autonomia,acoes:0,leituras:0,iniciada_em:agora(),fase:"aguardando".into(),cancelada:Arc::new(AtomicBool::new(false)),ocupada:false,ultima:None,pendente:None,resultados:vec![],_reserva:reserva,tipo,passos:vec![],foto:None});
  maquina::mostrar_barra(&app);
  if app.get_webview_window("barra-mac").is_none(){encerrar(&app,&mut g,"barra_indisponivel");return Err("Barra indisponível".into());}
  barra(&app,g.sessao.as_ref().unwrap());
  enviar(&g,json!({"t":"explorar-permissao","permitido":true,"id":id,"agente":agente,"bundle":bundle,"expira_em":ate,"autonomia":autonomia}));
 },
 "aprovar"|"recusar"=>{
  let s=g.sessao.as_mut().ok_or("sessao_encerrada")?;
  // A aprovação precisa do id que a barra acabou de mostrar; não pode aprovar
  // acidentalmente a próxima ação após um clique atrasado.
  let pending=s.pendente.as_ref().ok_or("sem_acao_pendente")?;
  if p["requisicao"]!=pending["requisicao"] {return Err("acao_ja_mudou".into());}
  let pending=s.pendente.take().unwrap();
  if acao=="recusar" {
   s.ultima=None;
   s.resultados.push((pending["requisicao"].as_str().unwrap().into(),json!({"ok":false,"motivo":"acao_recusada"})));
   registrar_passo(&mut s.passos,json!({"hora":agora(),"descricao":pending["passo"]["descricao"],"tipo":pending["passo"]["tipo"],"ok":false,"motivo":"recusada"}));
   if s.resultados.len()>2{s.resultados.remove(0);}barra(&app,s);
  } else {
   let requisicao=pending["requisicao"].as_str().unwrap().to_owned();
   if let Err(motivo)=iniciar(&app,&mut g,pending,true) {
    if let Some(s)=g.sessao.as_mut(){s.resultados.push((requisicao,json!({"ok":false,"motivo":motivo})));if s.resultados.len()>2{s.resultados.remove(0);}barra(&app,s);}
    return Err(motivo);
   }
  }
 },_=>return Err("acao_desconhecida".into())}
 Ok(estado(&g))
}
const TECLAS_NOMEADAS:&[&str]=&["escape","enter","tab","espaco","esquerda","direita","cima","baixo","backspace","delete","home","end","pageup","pagedown","f1","f2","f3","f4","f5","f6","f7","f8","f9","f10","f11","f12","down","up","left","right","return","space","esc","pgup","pgdown"];
fn validar_passo(p:&Value)->bool {
 let tipo=p["tipo"].as_str().unwrap_or("");
 if !["clique","passar","arrastar","rolar","texto","tecla","menu","esperar"].contains(&tipo)||p["descricao"].as_str().map(|s|s.is_empty()||s.len()>400).unwrap_or(true){return false;}
 if !["normal","confirmar"].contains(&p["risco"].as_str().unwrap_or("")){return false;}
 let coord=|k:&str|p[k].as_f64().map(|n|n.is_finite()&&(0.0..=1.0).contains(&n)).unwrap_or(false);
 let tecla_ok=|t:&str|TECLAS_NOMEADAS.contains(&t)||(t.chars().count()==1&&t.chars().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||"=-[];',./\\`".contains(c)));
 let mods_ok=p["mods"].as_array().map(|m|m.len()<=4&&m.iter().all(|x|["cmd","shift","alt","ctrl"].contains(&x.as_str().unwrap_or(""))) ).unwrap_or(p["mods"].is_null());
 match tipo {
  "clique"=>coord("x")&&coord("y")&&(p["cliques"].is_null()||[1,2].contains(&p["cliques"].as_i64().unwrap_or(0)))&&(p["botao"].is_null()||["esquerdo","direito"].contains(&p["botao"].as_str().unwrap_or(""))),
  "passar"=>coord("x")&&coord("y"),
  "arrastar"=>coord("x")&&coord("y")&&coord("x2")&&coord("y2"),
  "rolar"=>coord("x")&&coord("y")&&[&p["dy"],&p["dx"]].iter().all(|v|v.is_null()||v.as_i64().map(|n|(-600..=600).contains(&n)).unwrap_or(false))&&!(p["dy"].is_null()&&p["dx"].is_null()),
  "texto"=>p["texto"].as_str().map(|s|s.chars().count()<=1000&&!s.chars().any(char::is_control)).unwrap_or(false),
  "tecla"=>tecla_ok(&p["tecla"].as_str().unwrap_or("").to_lowercase())&&mods_ok,
  "menu"=>p["caminho"].as_array().map(|c|(if p["listar"]==true{0}else{1}..=4).contains(&c.len())&&c.iter().all(|x|x.as_str().map(|s|!s.trim().is_empty()&&s.len()<=80).unwrap_or(false))).unwrap_or(false)&&(p["listar"].is_null()||p["listar"].is_boolean()),
  "esperar"=>p["ms"].as_u64().map(|n|(100..=10_000).contains(&n)).unwrap_or(false),
  _=>false}
}
/// Combinado com o Rodrigo em 18/09: qualquer app (menos terminal, senhas e
/// Ajustes, barrados em `app_permitido`) e, no modo autônomo, o agente age
/// sozinho; só apagar, substituir, enviar/publicar e comprar pedem confirmação.
/// Editar dentro do app (cortar, mover, digitar) segue sozinho: a rodada exige
/// que a pessoa tenha preparado uma cópia de teste.
fn precisa_aprovar(s:&Sessao,p:&Value)->bool {
 let tipo=p["tipo"].as_str().unwrap_or("");
 if tipo=="esperar"||(tipo=="menu"&&p["listar"]==true){return false;}
 if !s.autonomia||p["risco"]!="normal"{return true;}
 // Cmd+Delete manda para o Lixo no Finder e apaga em vários apps.
 if tipo=="tecla"{let t=p["tecla"].as_str().unwrap_or("");let tem=|n:&str|p["mods"].as_array().map(|m|m.iter().any(|x|x==n)).unwrap_or(false);
  // Windows: Shift+Delete apaga sem passar pela Lixeira.
  if (tem("cmd")&&["backspace","delete"].contains(&t))||(tem("shift")&&t=="delete"){return true;}}
 // Defesa adicional: rótulos de ações sensíveis exigem aprovação mesmo que o
 // modelo as tenha classificado como normais. Não substitui revisão semântica.
 let mut rotulo=p["descricao"].as_str().unwrap_or("").to_lowercase();
 if let Some(c)=p["caminho"].as_array(){for x in c {rotulo.push(' ');rotulo.push_str(&x.as_str().unwrap_or("").to_lowercase());}}
 if let Some((_,_,leitura))=&s.ultima {
  let base=if leitura["captura"]["ret"].is_object(){&leitura["captura"]["ret"]}else{&leitura["janela"]["ret"]};
  if let (Some(x),Some(y),Some(r))=(p["x"].as_f64(),p["y"].as_f64(),base.as_object()) {
   let x=r["x"].as_f64().unwrap_or(0.0)+x*r["w"].as_f64().unwrap_or(0.0);let y=r["y"].as_f64().unwrap_or(0.0)+y*r["h"].as_f64().unwrap_or(0.0);
   if let Some(cs)=leitura["acessibilidade"]["controles"].as_array(){for c in cs {let a=&c["ret"];if let (Some(cx),Some(cy),Some(w),Some(h))=(a["x"].as_f64(),a["y"].as_f64(),a["w"].as_f64(),a["h"].as_f64()){if x>=cx&&x<=cx+w&&y>=cy&&y<=cy+h{rotulo.push_str(&format!(" {} {}",c["titulo"],c["descricao"]).to_lowercase());}}}}
  }
 }
 ["public","enviar","send","share","compartilh","apagar","exclu","delete","remover","lixo","trash","comprar","purchase","substitu","replace","pagamento","assinar"].iter().any(|v|rotulo.contains(v))
}
fn iniciar(app:&AppHandle,g:&mut Estado,v:Value,aprovada:bool)->Result<(),String>{
 let geracao=g.conexao.as_ref().ok_or("desconectado")?.0;
 let s=g.sessao.as_mut().ok_or("sem_permissao")?;
 if s.ate<=agora()||s.cancelada.load(Ordering::SeqCst)||v["sessao"]!=s.id||v["agente"]!=s.agente{return Err("sessao_nao_autorizada".into());}
 let req=v["requisicao"].as_str().filter(|s|uuid(s)).ok_or("requisicao_invalida")?.to_owned();
 if s.resultados.iter().any(|(r,_)|r==&req){return Err("requisicao_ja_executada_consulte_resultado".into());}
 if s.ocupada||s.pendente.is_some(){return Err("operacao_em_andamento".into());}
 let acao=v["acao"].as_str().unwrap_or("");let agir=acao=="agir";
 if !agir&&acao!="olhar"{return Err("acao_invalida".into());}
 if s.leituras>=500||s.acoes>=300{return Err("limite_da_sessao".into());}
 let mut entrada=None;let mut resumo:Option<Value>=None;
 if agir {
  let passo=&v["passo"];
  if !validar_passo(passo){return Err("passo_invalido".into());}
  let (ref id,ts,ref leitura)=s.ultima.as_ref().ok_or("observe_antes_de_agir")?;
  // Aprovada pela pessoa, a observação vale até 10 min: o ajudante só age se a
  // janela estiver pixel a pixel igual à observada (impressão), então esperar
  // a aprovação não abre brecha. Antes, aprovar depois de 2 min falhava calado.
  let validade=if aprovada{600_000}else{120_000};
  if v["observacao"]!=*id||agora()-ts>validade{return Err("observacao_expirada".into());}
  if !aprovada&&precisa_aprovar(s,passo){s.pendente=Some(v);barra(app,s);return Ok(());}
  let mut p=passo.clone();p["bundle"]=json!(s.bundle);p["janela"]=leitura["janela"]["numero"].clone();p["autonomo"]=json!(s.autonomia);p["captura"]=leitura["captura"]["ret"].clone();
  if !s.autonomia{p["impressao"]=leitura["impressao"].clone();}
  entrada=Some(p);
  s.ultima=None;s.acoes+=1;
  resumo=Some(json!({"descricao":passo["descricao"],"tipo":passo["tipo"],"aprovada":aprovada}));
 }
 s.fase=if agir{"agindo"}else{"observando"}.into();s.ocupada=true;s.leituras+=1;barra(app,s);
 let cancelada=s.cancelada.clone();let bundle=s.bundle.clone();let sessao=s.id.clone();let h=app.clone();let e=app.state::<Compartilhado>().inner().clone();
 std::thread::spawn(move||{
  let resultado=(||->Result<Value,String>{
   let mut menu=Value::Null;
   if let Some(p)=entrada {
    if p["tipo"]=="esperar"{
     // Espera o app (renderização, diálogo abrindo) sem o agente gastar um turno.
     let fim=Instant::now()+Duration::from_millis(p["ms"].as_u64().unwrap_or(500));
     while Instant::now()<fim{if cancelada.load(Ordering::SeqCst){return Err("pessoa".into());}std::thread::sleep(Duration::from_millis(100));}
    } else {
     let r=ajudante(&h,&["--acao"],Some(p),&cancelada)?;if r["ok"]!=true{return Err(r["motivo"].as_str().unwrap_or("acao_falhou").into());}
     menu=r["menu"].clone();std::thread::sleep(Duration::from_millis(350));
    }
   }
   let leitura=ajudante(&h,&["--explorar",&bundle],None,&cancelada)?;
   if leitura["ok"]!=true||leitura["consistente"]!=true{return Err(leitura["motivo"].as_str().unwrap_or("leitura_inconsistente").into());}
   Ok(json!({"ok":true,"leitura":leitura,"executada":agir,"observacao":req,"menu":menu}))
  })();
  if let Ok(mut g)=e.lock(){if g.conexao.as_ref().map(|c|c.0)!=Some(geracao){return;}
   let Some(s)=g.sessao.as_mut() else{return};if s.id!=sessao||!Arc::ptr_eq(&s.cancelada,&cancelada)||cancelada.load(Ordering::SeqCst)||s.ate<=agora(){return;}
   s.ocupada=false;s.fase="aguardando_leitura".into();let resposta=resultado.unwrap_or_else(|motivo|json!({"ok":false,"motivo":motivo,"acao_pode_ter_ocorrido":agir}));
   if resposta["ok"]==true{s.ultima=Some((req.clone(),agora(),resposta["leitura"].clone()));
    if resposta["leitura"]["foto"]["dados"].is_string(){s.foto=Some((agora(),resposta["leitura"]["foto"].clone()));}}
   if let Some(mut r)=resumo{r["hora"]=json!(agora());r["ok"]=json!(resposta["ok"]==true);if resposta["ok"]!=true{r["motivo"]=resposta["motivo"].clone();}registrar_passo(&mut s.passos,r);}
   s.resultados.push((req.clone(),resposta));if s.resultados.len()>2{s.resultados.remove(0);}barra(&h,s);
  };
 });Ok(())
}
pub fn receber(app:&AppHandle,geracao:u64,v:&Value){
 let e=app.state::<Compartilhado>();let Ok(mut g)=e.lock()else{return};
 if g.conexao.as_ref().map(|c|c.0)!=Some(geracao){return;}
 let Some(s)=g.sessao.as_ref()else{return};if v["sessao"]!=s.id||v["agente"]!=s.agente{return;}
 if s.ate<=agora(){encerrar(app,&mut g,"tempo_esgotado");return;}
 if v["acao"]=="fechar"{let motivo=v["motivo"].as_str().unwrap_or("concluida");encerrar(app,&mut g,motivo);return;}
 if v["acao"]=="situacao" {
  if let Some(fase)=v["fase"].as_str().filter(|f|["interpretando","decidindo"].contains(f)) {
   if let Some(s)=g.sessao.as_mut(){s.fase=fase.into();barra(app,s);}
  }return;
 }
 let resultado=if v["acao"]=="consultar" {
  let refid=v["operacao"].as_str().unwrap_or("");
  s.resultados.iter().find(|(id,_)|id==refid).map(|(_,r)|r.clone()).unwrap_or_else(||json!({"ok":true,"pendente":true,"aguarda_aprovacao":s.pendente.is_some()}))
 } else {match iniciar(app,&mut g,v.clone(),false){Ok(())=>json!({"ok":true,"pendente":true}),Err(m)=>json!({"ok":false,"motivo":m})}};
 enviar(&g,json!({"t":"explorar-resposta","sessao":v["sessao"],"requisicao":v["requisicao"],"resultado":resultado}));
}
pub fn conectar(app:&AppHandle,geracao:u64,tx:UnboundedSender<String>){
 if app.state::<maquina::NoMacCompartilhado>().lock().map(|n|n.geracao!=geracao).unwrap_or(true){return;}
 if let Ok(mut g)=app.state::<Compartilhado>().lock(){encerrar(app,&mut g,"conexao_substituida");g.conexao=Some((geracao,tx));}
}
pub fn desconectar(app:&AppHandle,geracao:u64){if let Ok(mut g)=app.state::<Compartilhado>().lock(){if g.conexao.as_ref().map(|c|c.0)==Some(geracao){encerrar(app,&mut g,"computador_desconectado");g.conexao=None;}}}
pub fn invalidar(app:&AppHandle){if let Ok(mut g)=app.state::<Compartilhado>().lock(){encerrar(app,&mut g,"identidade_alterada");g.conexao=None;}}
pub fn pode_enviar(app:&AppHandle,raw:&str)->bool{let Ok(v)=serde_json::from_str::<Value>(raw)else{return false};if v["t"]!="explorar-resposta"{return true;}app.state::<Compartilhado>().lock().map(|g|g.sessao.as_ref().map(|s|s.id==v["sessao"]&&s.ate>agora()&&!s.cancelada.load(Ordering::SeqCst)).unwrap_or(false)).unwrap_or(false)}
fn ajudante(app:&AppHandle,args:&[&str],entrada:Option<Value>,cancelada:&AtomicBool)->Result<Value,String>{
 if AJUDANTE.is_empty(){return Err("sistema_nao_suportado".into());}
 let dir=app.path().app_data_dir().map_err(|_|"pasta_indisponivel")?.join("ajudantes");std::fs::create_dir_all(&dir).map_err(|_|"pasta_indisponivel")?;
 let arq=dir.join(if cfg!(windows){format!("dnos-computador-win-{}.exe",app.package_info().version)}else{format!("dnos-explorar-mac-{}",app.package_info().version)});
 if std::fs::read(&arq).ok().as_deref()!=Some(AJUDANTE){std::fs::write(&arq,AJUDANTE).map_err(|_|"ajudante_indisponivel")?;#[cfg(unix)] {use std::os::unix::fs::PermissionsExt;std::fs::set_permissions(&arq,std::fs::Permissions::from_mode(0o700)).map_err(|_|"ajudante_indisponivel")?;}}
 if cancelada.load(Ordering::SeqCst){return Err("pessoa".into());}
 let mut f=sem_console(Command::new(&arq).args(args)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_|"ajudante_nao_iniciou")?;
 if let Some(v)=entrada{if let Some(mut i)=f.stdin.take(){let _=i.write_all(v.to_string().as_bytes());}}else{drop(f.stdin.take());}
 let out=f.stdout.take().ok_or("sem_saida")?;let leitor=std::thread::spawn(move||{let mut b=vec![];out.take(3_000_001).read_to_end(&mut b).map(|_|b)});let inicio=Instant::now();
 let erro=loop{if cancelada.load(Ordering::SeqCst){break Some("pessoa");}if inicio.elapsed()>Duration::from_secs(10){break Some("ajudante_sem_resposta");}match f.try_wait(){Ok(Some(s))=>break if s.success(){None}else{Some("ajudante_falhou")},Err(_)=>break Some("ajudante_falhou"),_=>{}}std::thread::sleep(Duration::from_millis(20));};
 let _=f.kill();let _=f.wait();if erro.is_some() && args==["--acao"] {let _=sem_console(Command::new(&arq).arg("--soltar")).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();}let b=leitor.join().map_err(|_|"saida_indisponivel")?.map_err(|_|"saida_indisponivel")?;
 if let Some(e)=erro{return Err(e.into());}if b.len()>3_000_000{return Err("leitura_muito_grande".into());}serde_json::from_slice(&b).map_err(|_|"saida_invalida".into())
}
pub fn instalar(app:&AppHandle){app.manage::<Compartilhado>(Arc::new(Mutex::new(Estado::default())));let h=app.clone();std::thread::spawn(move||loop{std::thread::sleep(Duration::from_millis(200));if let Ok(mut g)=h.state::<Compartilhado>().lock(){if g.sessao.as_ref().map(|s|s.ate<=agora()).unwrap_or(false){encerrar(&h,&mut g,"tempo_esgotado");}else if let Some(s)=g.sessao.as_mut(){if s.fase=="aguardando"&&agora()-s.iniciada_em>60_000{s.fase="demorado".into();barra(&h,s);}}}});}
#[cfg(test)] mod testes{use super::*;
 #[test]fn rejeita_scripts_e_coordenadas_invalidas(){assert!(!validar_passo(&json!({"tipo":"shell","descricao":"testar","risco":"normal"})));assert!(!validar_passo(&json!({"tipo":"clique","descricao":"x","risco":"normal","x":2,"y":0})));assert!(validar_passo(&json!({"tipo":"clique","descricao":"abrir painel","risco":"normal","x":0.5,"y":0.5})));assert!(!validar_passo(&json!({"tipo":"texto","descricao":"colar","risco":"normal","texto":"a\nb"})));}
 #[test]fn aceita_gestos_novos_e_recusa_malformados(){
  let ok=|v:Value|validar_passo(&v);
  assert!(ok(json!({"tipo":"clique","descricao":"abrir","risco":"normal","x":0.5,"y":0.5,"cliques":2})));
  assert!(ok(json!({"tipo":"clique","descricao":"menu contexto","risco":"normal","x":0.5,"y":0.5,"botao":"direito"})));
  assert!(ok(json!({"tipo":"tecla","descricao":"dividir clipe","risco":"normal","tecla":"b","mods":["cmd"]})));
  assert!(ok(json!({"tipo":"menu","descricao":"exportar","risco":"normal","caminho":["Arquivo","Exportar"]})));
  assert!(ok(json!({"tipo":"esperar","descricao":"renderizar","risco":"normal","ms":3000})));
  assert!(ok(json!({"tipo":"rolar","descricao":"timeline","risco":"normal","x":0.5,"y":0.8,"dx":-200})));
  assert!(!ok(json!({"tipo":"clique","descricao":"x","risco":"normal","x":0.5,"y":0.5,"cliques":3})));
  assert!(!ok(json!({"tipo":"tecla","descricao":"x","risco":"normal","tecla":"b","mods":["hyper"]})));
  assert!(!ok(json!({"tipo":"tecla","descricao":"x","risco":"normal","tecla":"rm -rf"})));
  assert!(!ok(json!({"tipo":"menu","descricao":"x","risco":"normal","caminho":[]})));
  assert!(ok(json!({"tipo":"menu","descricao":"menus do topo","risco":"normal","caminho":[],"listar":true})));
  assert!(!ok(json!({"tipo":"esperar","descricao":"x","risco":"normal","ms":60000})));
 }
 #[test]fn guarda_so_os_ultimos_passos(){
  let mut p=vec![];for i in 0..(MAX_PASSOS+5){registrar_passo(&mut p,json!({"n":i}));}
  assert_eq!(p.len(),MAX_PASSOS);assert_eq!(p[0]["n"],5);assert_eq!(p[MAX_PASSOS-1]["n"],MAX_PASSOS+4);
 }
 #[test]fn nao_oferece_terminais_ou_cofres(){assert!(app_permitido("com.lemon.lvoverseas"));assert!(!app_permitido("com.apple.Terminal"));assert!(!app_permitido("com.apple.keychainaccess"));}
 #[test]fn executaveis_do_windows(){
  assert!(app_permitido("CapCut.exe"));assert!(app_permitido("Adobe Premiere Pro.exe"));assert!(app_permitido("acro_rd32.exe"));assert!(app_permitido("explorer.exe"));
  for barrado in ["cmd.exe","PowerShell.exe","pwsh.exe","WindowsTerminal.exe","wt.exe","regedit.exe","KeePassXC.exe","Bitwarden.exe","SystemSettings.exe","dnos-desktop.exe"] {assert!(!app_permitido(barrado),"{barrado} deveria ser barrado");}
  assert!(!app_permitido("app;calc.exe"));assert!(!app_permitido("a\\b.exe"));
 }
}
