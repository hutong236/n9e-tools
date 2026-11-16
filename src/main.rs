mod excel;
use axum::{extract::{Multipart,DefaultBodyLimit,State},http::StatusCode,response::Html,routing::{get,post},Json,Router};
use serde::{Deserialize,Serialize};
use serde_json::Value;
use std::{collections::{HashSet,HashMap},net::SocketAddr,time::Duration,sync::{Arc,Mutex}};
use excel::Preview;
type AppState=Arc<Mutex<HashMap<String,Preview>>>;

#[derive(Deserialize)]struct ImportRequest { url:String,token:String,group_id:u64,datasource_id:u64,preview_id:String,rows:Vec<u32>,confirm:bool }
#[derive(Serialize)]struct ResultRow{name:String,status:String,message:String}
#[derive(Serialize)]struct ImportResult{created:usize,skipped:usize,failed:usize,items:Vec<ResultRow>}
fn error(code:StatusCode,msg:impl Into<String>)->(StatusCode,Json<Value>){(code,Json(serde_json::json!({"error":msg.into()})))}
async fn index()->Html<&'static str>{Html(include_str!("../static/index.html"))}
async fn preview(State(state):State<AppState>,mut multipart:Multipart)->Result<Json<Preview>,(StatusCode,Json<Value>)>{
 let mut file=None;
 while let Some(field)=multipart.next_field().await.map_err(|e|error(StatusCode::BAD_REQUEST,e.to_string()))?{
  if field.name()==Some("file"){file=Some(field.bytes().await.map_err(|e|error(StatusCode::BAD_REQUEST,e.to_string()))?);break}
 }
 let bytes=file.ok_or_else(||error(StatusCode::BAD_REQUEST,"缺少 file 字段"))?;
 let mut preview=tokio::task::spawn_blocking(move||excel::parse(&bytes)).await.map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?.map_err(|e|error(StatusCode::BAD_REQUEST,e))?;
 preview.preview_id=uuid::Uuid::new_v4().to_string();
 {
  let mut cache=state.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"状态锁异常"))?;
  if cache.len()>20 {cache.clear();}
  cache.insert(preview.preview_id.clone(),Preview{preview_id:preview.preview_id.clone(),sheets:preview.sheets.clone(),selected:preview.selected,all_rules:preview.all_rules,rules:preview.rules.clone(),workbook_warnings:preview.workbook_warnings.clone()});
 }
 Ok(Json(preview))
}
fn valid_url(raw:&str)->Result<String,String>{
 let u=reqwest::Url::parse(raw).map_err(|e|format!("夜莺URL无效: {e}"))?;
 if !matches!(u.scheme(),"http"|"https")||u.username()!=""||u.password().is_some()||u.query().is_some()||u.fragment().is_some(){return Err("仅允许无用户信息、查询和片段的 http(s) URL".into())}
 Ok(raw.trim_end_matches('/').to_string())
}
fn extract_list(v:&Value)->Result<&Vec<Value>,String>{
 if let Some(arr)=v.as_array(){return Ok(arr)}
 for key in ["dat","data"]{if let Some(a)=v.get(key).and_then(Value::as_array){return Ok(a)}
  if let Some(a)=v.get(key).and_then(|x|x.get("list")).and_then(Value::as_array){return Ok(a)}
 }
 Err("无法识别现有规则列表结构；为避免重复创建，已中止".into())
}
fn check_api(v:&Value)->Result<(),String>{
 if let Some(e)=v.get("err"){if !e.is_null()&&e!=0&&e!=""{return Err(format!("夜莺API业务错误: {e}"))}}
 if let Some(c)=v.get("code").and_then(Value::as_i64){if c!=0&&c!=200{return Err(format!("API code={c}"))}}
 Ok(())
}
async fn api_json(resp:reqwest::Response)->Result<Value,String>{
 let status=resp.status();let ct=resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|x|x.to_str().ok()).unwrap_or("").to_string();
 if !status.is_success(){return Err(format!("HTTP {status}"))}
 if !ct.contains("json"){return Err(format!("响应不是JSON（{ct}）；确认API路径和认证"))}
 let v=resp.json::<Value>().await.map_err(|e|e.to_string())?;check_api(&v)?;Ok(v)
}
async fn import(State(state):State<AppState>,Json(req):Json<ImportRequest>)->Result<Json<ImportResult>,(StatusCode,Json<Value>)>{
 if !req.confirm{return Err(error(StatusCode::BAD_REQUEST,"必须先确认导入"))}
 if req.token.trim().is_empty()||req.group_id==0||req.datasource_id==0{return Err(error(StatusCode::BAD_REQUEST,"Token、业务组ID和数据源ID必填"))}
 if req.rows.is_empty()||req.rows.len()>54{return Err(error(StatusCode::BAD_REQUEST,"只能导入预览中选定的1-54条规则"))}
 let rules={let cache=state.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"状态锁异常"))?;
  let preview=cache.get(&req.preview_id).ok_or_else(||error(StatusCode::BAD_REQUEST,"预览已失效，请重新上传 Excel"))?;
  let mut seen=HashSet::new();
  if req.rows.iter().any(|row|!seen.insert(*row)){return Err(error(StatusCode::BAD_REQUEST,"重复选择了相同行"))}
  let mut rules=Vec::new();
  for row in &req.rows {let r=preview.rules.iter().find(|r|r.row==*row).ok_or_else(||error(StatusCode::BAD_REQUEST,"所选行不属于已解析的红色规则"))?;
   rules.push(r.clone());}
  rules
 };
 let base=valid_url(&req.url).map_err(|e|error(StatusCode::BAD_REQUEST,e))?;
 let client=reqwest::Client::builder().timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
 let uri=format!("{base}/api/n9e/busi-group/{}/alert-rules",req.group_id);
 let response=client.get(&uri).header("X-User-Token",&req.token).send().await.map_err(|e|error(StatusCode::BAD_GATEWAY,e.to_string()))?;
 let existing=api_json(response).await.map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 let existing=extract_list(&existing).map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 let mut names:HashSet<String>=existing.iter().filter_map(|x|x.get("name").and_then(Value::as_str).map(str::to_owned)).collect();
 let mut results=ImportResult{created:0,skipped:0,failed:0,items:vec![]};
 for r in &rules {
  if names.contains(&r.name){results.skipped+=1;results.items.push(ResultRow{name:r.name.clone(),status:"skipped".into(),message:"同业务组内已存在同名规则".into()});continue}
  if excel::seconds(&r.duration).is_none()||r.promql.is_empty()||r.name.is_empty()||r.issues.iter().any(|i|i.level=="error"){
   results.failed+=1;results.items.push(ResultRow{name:r.name.clone(),status:"invalid".into(),message:"规则校验不通过".into()});continue
  }
  let payload=serde_json::json!({
   "name":r.name,"note":r.note,
   "group_id":req.group_id,"datasource_queries":[{"match_type":0,"op":"in","values":[req.datasource_id]}],
   "cate":"prometheus","prod":"metric","severity":1,"disabled":1,
   "prom_for_duration":r.duration_seconds,"prom_eval_interval":30,"prom_ql":"",
   "enable_stimes":["00:00"],"enable_etimes":["00:00"],
   "enable_days_of_weeks":[["0","1","2","3","4","5","6"]],
   "annotations":{"summary":r.summary,"description":r.description,"suggestion":r.suggestion},
   "append_tags":[],"notify_rule_ids":[],
   "rule_config":{"queries":[{"prom_ql":r.promql,"severity":1}]},
   "notify_repeat_step":0,"notify_max_number":0,"recover_duration":0
  });
  let res=client.post(&uri).header("X-User-Token",&req.token).json(&vec![payload]).send().await;
  let outcome=match res{Ok(response)=>api_json(response).await.and_then(|v|{let d=v.get("dat").or_else(||v.get("data")).ok_or("创建接口缺少 dat，无法确认写入结果")?;match d.get(&r.name).and_then(Value::as_str){Some("")=>Ok(()),Some(msg)=>Err(format!("夜莺拒绝规则：{msg}")),None=>Err("创建接口未返回该规则的处理结果".into())}}),Err(e)=>Err(e.to_string())};
  match outcome{Ok(())=>{results.created+=1;names.insert(r.name.clone());results.items.push(ResultRow{name:r.name.clone(),status:"created".into(),message:"API已接受创建请求（默认禁用）".into()})},Err(e)=>{results.failed+=1;results.items.push(ResultRow{name:r.name.clone(),status:"failed".into(),message:e})}}
 }
 Ok(Json(results))
}
#[tokio::main]async fn main(){
 tracing_subscriber::fmt::init();
 let state:AppState=Arc::new(Mutex::new(HashMap::new()));
 let app=Router::new().route("/",get(index)).route("/api/preview",post(preview)).route("/api/import",post(import)).layer(DefaultBodyLimit::max(16*1024*1024)).with_state(state);
 let bind=std::env::var("BIND").unwrap_or("127.0.0.1:8088".into());
 let addr:SocketAddr=bind.parse().expect("BIND must be IP:port");
 let listener=tokio::net::TcpListener::bind(addr).await.unwrap();
 println!("Web http://{addr}");
 axum::serve(listener,app).await.unwrap();
}