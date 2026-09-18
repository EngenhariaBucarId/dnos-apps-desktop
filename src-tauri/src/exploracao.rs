//! Sessão de aprendizado: um agente, aplicativo e rodada escolhidos pela pessoa.
//! Cada ação consome uma observação. Nunca aceita shell, scripts ou caminhos.
use serde_json::{json,Value};
use std::{io::{Read,Write},process::{Command,Stdio},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
use tauri::{AppHandle,Manager};
use tokio::sync::mpsc::UnboundedSender;
use crate::maquina::{self,ReservaDeUso};
#[cfg(target_os="macos")] const AJUDANTE:&[u8]=include_bytes!("../ajudantes/dnos-olhar-mac");
#[cfg(not(target_os="macos"))] const AJUDANTE:&[u8]=&[];
struct Sessao { id:String,agente:String,nome:String,app_nome:String,bundle:String,ate:u64,autonomia:bool,acoes:u32,leituras:u32,
 cancelada:Arc<AtomicBool>,ocupada:bool,ultima:Option<(String,u64,Value)>,pendente:Option<Value>,resultados:Vec<(String,Value)>,_reserva:ReservaDeUso }
#[derive(Default)] pub struct Estado { conexao:Option<(u64,UnboundedSender<String>)>,sessao:Option<Sessao> }
type Compartilhado=Arc<Mutex<Estado>>;
fn agora()->u64 {crate::gravador::agora_ms() as u64}
fn uuid(s:&str)->bool {s.len()==36 && s.bytes().all(|c|c.is_ascii_hexdigit()||c==b'-')}
fn app_permitido(s:&str)->bool {
 !s.is_empty() && s.len()<200 && s.bytes().all(|c|c.is_ascii_alphanumeric()||b".-".contains(&c))
 && !["terminal","iterm","keychain","systempreferences","password","1password","dnos"].iter().any(|x|s.to_lowercase().contains(x))
}
fn enviar(g:&Estado,v:Value) {if let Some((_,tx))=&g.conexao {let _=tx.send(v.to_string());}}
fn encerrar(app:&AppHandle,g:&mut Estado,motivo:&str) {
 if let Some(s)=g.sessao.take(){s.cancelada.store(true,Ordering::SeqCst);enviar(g,json!({"t":"explorar-fim","sessao":s.id,"motivo":motivo}));enviar(g,json!({"t":"explorar-permissao","permitido":false}));maquina::esconder_barra(app);}
}
fn estado(g:&Estado)->Value {match &g.sessao {
 Some(s)=>json!({"disponivel":!AJUDANTE.is_empty(),"conectado":g.conexao.is_some(),"ativo":true,"id":s.id,"agente":s.agente,"bundle":s.bundle,"expira_em":s.ate,"acoes":s.acoes,"pendente":s.pendente}),
 None=>json!({"disponivel":!AJUDANTE.is_empty(),"conectado":g.conexao.is_some(),"ativo":false})}}
fn barra(app:&AppHandle,s:&Sessao) {
 let sub=if let Some(p)=&s.pendente {format!("Aprovar: {}",p["passo"]["descricao"].as_str().unwrap_or("próxima ação"))} else {format!("{} ações de 40 · {} · Parar encerra",s.acoes,if s.autonomia{"autonomia autorizada"}else{"ações acompanhadas"})};
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
 "autorizar"=>{
  if AJUDANTE.is_empty()||g.conexao.is_none(){return Err("Abra o Desktop atualizado e espere a conexão.".into());}
  if g.sessao.is_some(){return Err("Encerre a sessão anterior.".into());}
  let id=p["id"].as_str().filter(|s|uuid(s)).ok_or("rodada_obrigatoria")?.to_owned();
  let agente=p["agente"].as_str().filter(|s|!s.is_empty()&&s.len()<=100).ok_or("agente_obrigatorio")?.to_owned();
  let nome=p["nome"].as_str().unwrap_or(&agente).chars().take(100).collect::<String>();
  let bundle=p["bundle"].as_str().filter(|s|app_permitido(s)).ok_or("aplicativo_nao_permitido")?.to_owned();
  let app_nome=p["app_nome"].as_str().unwrap_or(&bundle).chars().take(100).collect::<String>();
  let minutos=p["minutos"].as_u64().filter(|m|[5,10,15].contains(m)).ok_or("prazo_invalido")?;
  let autonomia=p["autonomia"].as_bool().unwrap_or(false);let ate=agora()+minutos*60_000;
  let reserva=maquina::reservar_uso(&app,"explorando")?;
  g.sessao=Some(Sessao{id:id.clone(),agente:agente.clone(),nome,app_nome,bundle:bundle.clone(),ate,autonomia,acoes:0,leituras:0,cancelada:Arc::new(AtomicBool::new(false)),ocupada:false,ultima:None,pendente:None,resultados:vec![],_reserva:reserva});
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
fn validar_passo(p:&Value)->bool {
 let tipo=p["tipo"].as_str().unwrap_or("");
 if !["clique","arrastar","rolar","texto","tecla"].contains(&tipo)||p["descricao"].as_str().map(|s|s.is_empty()||s.len()>400).unwrap_or(true){return false;}
 if !["normal","confirmar"].contains(&p["risco"].as_str().unwrap_or("")){return false;}
 let coord=|k:&str|p[k].as_f64().map(|n|n.is_finite()&&(0.0..=1.0).contains(&n)).unwrap_or(false);
 match tipo {"clique"=>coord("x")&&coord("y"),"arrastar"=>coord("x")&&coord("y")&&coord("x2")&&coord("y2"),"rolar"=>coord("x")&&coord("y")&&p["dy"].as_i64().map(|n|(-600..=600).contains(&n)).unwrap_or(false),"texto"=>p["texto"].as_str().map(|s|s.chars().count()<=1000&&!s.chars().any(char::is_control)).unwrap_or(false),"tecla"=>["escape","enter","tab","espaco","esquerda","direita","cima","baixo","backspace"].contains(&p["tecla"].as_str().unwrap_or("")),_=>false}
}
fn precisa_aprovar(s:&Sessao,p:&Value)->bool {
 if !s.autonomia||p["risco"]!="normal"||["texto","tecla"].contains(&p["tipo"].as_str().unwrap_or("")){return true;}
 // Defesa adicional: rótulos de ações sensíveis exigem aprovação mesmo que o
 // modelo as tenha classificado como normais. Não substitui revisão semântica.
 let mut rotulo=p["descricao"].as_str().unwrap_or("").to_lowercase();
 if let Some((_,_,leitura))=&s.ultima {
  if let (Some(x),Some(y),Some(r))=(p["x"].as_f64(),p["y"].as_f64(),leitura["janela"]["ret"].as_object()) {
   let x=r["x"].as_f64().unwrap_or(0.0)+x*r["w"].as_f64().unwrap_or(0.0);let y=r["y"].as_f64().unwrap_or(0.0)+y*r["h"].as_f64().unwrap_or(0.0);
   if let Some(cs)=leitura["acessibilidade"]["controles"].as_array(){for c in cs {let a=&c["ret"];if let (Some(cx),Some(cy),Some(w),Some(h))=(a["x"].as_f64(),a["y"].as_f64(),a["w"].as_f64(),a["h"].as_f64()){if x>=cx&&x<=cx+w&&y>=cy&&y<=cy+h{rotulo.push_str(&format!(" {} {}",c["titulo"],c["descricao"]).to_lowercase());}}}}
  }
 }
 ["export","public","enviar","send","share","compartilh","apagar","exclu","delete","remover","comprar","purchase","substitu","replace","pagamento"].iter().any(|v|rotulo.contains(v))
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
 if s.leituras>=80||s.acoes>=40{return Err("limite_da_sessao".into());}
 let mut entrada=None;
 if agir {
  let passo=&v["passo"];
  if !validar_passo(passo){return Err("passo_invalido".into());}
  let (ref id,ts,ref leitura)=s.ultima.as_ref().ok_or("observe_antes_de_agir")?;
  if v["observacao"]!=*id||agora()-ts>120_000{return Err("observacao_expirada".into());}
  if !aprovada&&precisa_aprovar(s,passo){s.pendente=Some(v);barra(app,s);return Ok(());}
  let mut p=passo.clone();p["bundle"]=json!(s.bundle);p["impressao"]=leitura["impressao"].clone();p["janela"]=leitura["janela"]["numero"].clone();entrada=Some(p);
  s.ultima=None;s.acoes+=1;
 }
 s.ocupada=true;s.leituras+=1;barra(app,s);
 let cancelada=s.cancelada.clone();let bundle=s.bundle.clone();let sessao=s.id.clone();let h=app.clone();let e=app.state::<Compartilhado>().inner().clone();
 std::thread::spawn(move||{
  let resultado=(||->Result<Value,String>{
   if let Some(p)=entrada {let r=ajudante(&h,&["--acao"],Some(p),&cancelada)?;if r["ok"]!=true{return Err(r["motivo"].as_str().unwrap_or("acao_falhou").into());}std::thread::sleep(Duration::from_millis(350));}
   let leitura=ajudante(&h,&["--explorar",&bundle],None,&cancelada)?;
   if leitura["ok"]!=true||leitura["consistente"]!=true{return Err(leitura["motivo"].as_str().unwrap_or("leitura_inconsistente").into());}
   Ok(json!({"ok":true,"leitura":leitura,"executada":agir,"observacao":req}))
  })();
  if let Ok(mut g)=e.lock(){if g.conexao.as_ref().map(|c|c.0)!=Some(geracao){return;}
   let Some(s)=g.sessao.as_mut() else{return};if s.id!=sessao||!Arc::ptr_eq(&s.cancelada,&cancelada)||cancelada.load(Ordering::SeqCst)||s.ate<=agora(){return;}
   s.ocupada=false;let resposta=resultado.unwrap_or_else(|motivo|json!({"ok":false,"motivo":motivo,"acao_pode_ter_ocorrido":agir}));
   if resposta["ok"]==true{s.ultima=Some((req.clone(),agora(),resposta["leitura"].clone()));}
   s.resultados.push((req.clone(),resposta));if s.resultados.len()>2{s.resultados.remove(0);}barra(&h,s);
  };
 });Ok(())
}
pub fn receber(app:&AppHandle,geracao:u64,v:&Value){
 let e=app.state::<Compartilhado>();let Ok(mut g)=e.lock()else{return};
 if g.conexao.as_ref().map(|c|c.0)!=Some(geracao){return;}
 let Some(s)=g.sessao.as_ref()else{return};if v["sessao"]!=s.id||v["agente"]!=s.agente{return;}
 if s.ate<=agora(){encerrar(app,&mut g,"tempo_esgotado");return;}
 if v["acao"]=="fechar"{encerrar(app,&mut g,"concluida");return;}
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
 let arq=dir.join(format!("dnos-explorar-mac-{}",app.package_info().version));
 if std::fs::read(&arq).ok().as_deref()!=Some(AJUDANTE){std::fs::write(&arq,AJUDANTE).map_err(|_|"ajudante_indisponivel")?;#[cfg(unix)] {use std::os::unix::fs::PermissionsExt;std::fs::set_permissions(&arq,std::fs::Permissions::from_mode(0o700)).map_err(|_|"ajudante_indisponivel")?;}}
 if cancelada.load(Ordering::SeqCst){return Err("pessoa".into());}
 let mut f=Command::new(&arq).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_|"ajudante_nao_iniciou")?;
 if let Some(v)=entrada{if let Some(mut i)=f.stdin.take(){let _=i.write_all(v.to_string().as_bytes());}}else{drop(f.stdin.take());}
 let out=f.stdout.take().ok_or("sem_saida")?;let leitor=std::thread::spawn(move||{let mut b=vec![];out.take(3_000_001).read_to_end(&mut b).map(|_|b)});let inicio=Instant::now();
 let erro=loop{if cancelada.load(Ordering::SeqCst){break Some("pessoa");}if inicio.elapsed()>Duration::from_secs(10){break Some("ajudante_sem_resposta");}match f.try_wait(){Ok(Some(s))=>break if s.success(){None}else{Some("ajudante_falhou")},Err(_)=>break Some("ajudante_falhou"),_=>{}}std::thread::sleep(Duration::from_millis(20));};
 let _=f.kill();let _=f.wait();if erro.is_some() && args==["--acao"] {let _=Command::new(&arq).arg("--soltar").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();}let b=leitor.join().map_err(|_|"saida_indisponivel")?.map_err(|_|"saida_indisponivel")?;
 if let Some(e)=erro{return Err(e.into());}if b.len()>3_000_000{return Err("leitura_muito_grande".into());}serde_json::from_slice(&b).map_err(|_|"saida_invalida".into())
}
pub fn instalar(app:&AppHandle){app.manage::<Compartilhado>(Arc::new(Mutex::new(Estado::default())));let h=app.clone();std::thread::spawn(move||loop{std::thread::sleep(Duration::from_millis(200));if let Ok(mut g)=h.state::<Compartilhado>().lock(){if g.sessao.as_ref().map(|s|s.ate<=agora()).unwrap_or(false){encerrar(&h,&mut g,"tempo_esgotado");}}});}
#[cfg(test)] mod testes{use super::*;
 #[test]fn rejeita_scripts_e_coordenadas_invalidas(){assert!(!validar_passo(&json!({"tipo":"shell","descricao":"testar","risco":"normal"})));assert!(!validar_passo(&json!({"tipo":"clique","descricao":"x","risco":"normal","x":2,"y":0})));assert!(validar_passo(&json!({"tipo":"clique","descricao":"abrir painel","risco":"normal","x":0.5,"y":0.5})));assert!(!validar_passo(&json!({"tipo":"texto","descricao":"colar","risco":"normal","texto":"a\nb"})));}
 #[test]fn nao_oferece_terminais_ou_cofres(){assert!(app_permitido("com.lemon.lvoverseas"));assert!(!app_permitido("com.apple.Terminal"));assert!(!app_permitido("com.apple.keychainaccess"));}
}
