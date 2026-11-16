mod excel;
use axum::{body::{Body,Bytes},extract::{Multipart,DefaultBodyLimit,State},http::{StatusCode,header},response::{Html,Response},routing::{get,post},Json,Router};
use serde::{Deserialize,Serialize};
use serde_json::Value;
use std::{collections::{HashSet,HashMap},net::SocketAddr,time::Duration,sync::{Arc,Mutex}};
use excel::Preview;
use tokio_stream::{wrappers::ReceiverStream,StreamExt};
type AppState=Arc<Mutex<HashMap<String,Preview>>>;

#[derive(Clone,Deserialize)]struct ImportRequest { url:String,token:String,group_id:u64,datasource_id:u64,preview_id:String,rows:Vec<u32>,confirm:bool }
#[derive(Deserialize)]struct TestConnectionRequest { url:String,token:String,group_id:u64,datasource_id:u64 }
#[derive(Serialize)]struct ConnectionResult { connected:bool, message:String, existing_rules:usize, group_id:u64, datasource_id:u64, datasource_checked:bool }

#[derive(Deserialize)]struct DiscoverRequest { url:String,token:String }
#[derive(Serialize)]struct OptionItem { id:u64,name:String,kind:String }
#[derive(Serialize)]struct DiscoverResult {groups:Vec<OptionItem>,datasources:Vec<OptionItem>}
fn parse_options(value:&Value,category:&str)->Result<Vec<OptionItem>,String>{
 let arr=extract_list(value)?;
 let mut out=Vec::new();
 for item in arr{
  let id=item.get("id").or_else(||item.get("Id")).and_then(|x|x.as_u64().or_else(||x.as_str().and_then(|v|v.parse::<u64>().ok())));
  let name=item.get("name").or_else(||item.get("Name")).and_then(Value::as_str);
  if let (Some(id),Some(name))=(id,name){
   if id>0&&!name.trim().is_empty(){
    let kind=item.get("plugin_type").or_else(||item.get("cate")).and_then(Value::as_str).unwrap_or("");
    out.push(OptionItem{id,name:name.to_string(),kind:if category=="group"{String::new()}else{kind.to_string()}});
   }
  }
 }
 if !arr.is_empty()&&out.is_empty(){return Err(format!("{category} 数据格式无法识别 ID 与名称"))}
 out.sort_by(|a,b|a.name.cmp(&b.name));
 Ok(out)
}
async fn discover(Json(req):Json<DiscoverRequest>)->Result<Json<DiscoverResult>,(StatusCode,Json<Value>)>{
 if req.token.trim().is_empty(){return Err(error(StatusCode::BAD_REQUEST,"请填写 X-User-Token"))}
 let base=valid_url(&req.url).map_err(|e|error(StatusCode::BAD_REQUEST,e))?;
 let client=reqwest::Client::builder().timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).build().map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
 let group_uri=format!("{base}/api/n9e/busi-groups");
 let groups_response=client.get(group_uri).header("X-User-Token",&req.token).send().await.map_err(|e|error(StatusCode::BAD_GATEWAY,format!("业务组查询失败：{e}")))?;
 let groups_json=api_json(groups_response).await.map_err(|e|error(StatusCode::BAD_GATEWAY,format!("业务组接口异常：{e}")))?;
 let groups=parse_options(&groups_json,"group").map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 let datasource_uri=format!("{base}/api/n9e/datasource/list");
 let ds_response=client.post(datasource_uri).header("X-User-Token",&req.token)
    .json(&serde_json::json!({"name":"","plugin_type":"","category":""})).send().await
    .map_err(|e|error(StatusCode::BAD_GATEWAY,format!("数据源查询失败：{e}")))?;
 let ds_json=api_json(ds_response).await.map_err(|e|error(StatusCode::BAD_GATEWAY,format!("数据源接口异常：{e}")))?;
 // Only return id/name/type: never forward datasource credentials or settings to browser.
 let datasources=parse_options(&ds_json,"datasource").map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 Ok(Json(DiscoverResult{groups,datasources}))
}
#[derive(Clone,Serialize)]struct ResultRow{row:u32,name:String,status:String,message:String}
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
fn creation_result(v:&Value,name:&str)->Result<(),String>{
 let d=v.get("dat").or_else(||v.get("data")).ok_or("创建接口缺少 dat，无法确认写入结果")?;
 match d.get(name).and_then(Value::as_str){
  Some("")=>Ok(()),
  Some(msg)=>Err(format!("夜莺拒绝规则：{msg}")),
  None=>Err("创建接口未返回该规则的处理结果".into())
 }
}
async fn api_json(resp:reqwest::Response)->Result<Value,String>{
 let status=resp.status();let ct=resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|x|x.to_str().ok()).unwrap_or("").to_string();
 if !status.is_success(){return Err(format!("HTTP {status}"))}
 if !ct.contains("json"){return Err(format!("响应不是JSON（{ct}）；确认API路径和认证"))}
 let v=resp.json::<Value>().await.map_err(|e|e.to_string())?;check_api(&v)?;Ok(v)
}
async fn test_connection(Json(req):Json<TestConnectionRequest>)->Result<Json<ConnectionResult>,(StatusCode,Json<Value>)>{
 if req.token.trim().is_empty()||req.group_id==0||req.datasource_id==0{return Err(error(StatusCode::BAD_REQUEST,"Token、业务组 ID、数据源 ID 必填"))}
 let base=valid_url(&req.url).map_err(|e|error(StatusCode::BAD_REQUEST,e))?;
 let client=reqwest::Client::builder().timeout(Duration::from_secs(12)).redirect(reqwest::redirect::Policy::none()).build().map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
 let uri=format!("{base}/api/n9e/busi-group/{}/alert-rules",req.group_id);
 let resp=client.get(uri).header("X-User-Token",&req.token).send().await.map_err(|e|error(StatusCode::BAD_GATEWAY,format!("无法连接夜莺：{e}")))?;
 let value=api_json(resp).await.map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 let list=extract_list(&value).map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 Ok(Json(ConnectionResult{connected:true,message:"已通过 API 访问目标业务组告警规则；数据源 ID 尚未独立验证".into(),existing_rules:list.len(),group_id:req.group_id,datasource_id:req.datasource_id,datasource_checked:false}))
}
async fn emit_update(tx:&Option<tokio::sync::mpsc::Sender<Bytes>>, value:Value){
 if let Some(sender)=tx {
  if let Ok(mut bytes)=serde_json::to_vec(&value){
   bytes.push(b'\n');
   let _=sender.send(Bytes::from(bytes)).await;
  }
 }
}
async fn import_core(state:AppState,req:ImportRequest,updates:Option<tokio::sync::mpsc::Sender<Bytes>>)->Result<ImportResult,(StatusCode,Json<Value>)>{
 if !req.confirm{return Err(error(StatusCode::BAD_REQUEST,"必须先确认导入"))}
 if req.token.trim().is_empty()||req.group_id==0||req.datasource_id==0{return Err(error(StatusCode::BAD_REQUEST,"Token、业务组ID和数据源ID必填"))}
 if req.rows.is_empty()||req.rows.len()>10000{return Err(error(StatusCode::BAD_REQUEST,"请选择1至10000条有效规则"))}
 let rules={let cache=state.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"状态锁异常"))?;
  let preview=cache.get(&req.preview_id).ok_or_else(||error(StatusCode::BAD_REQUEST,"预览已失效，请重新上传 Excel"))?;
  let mut seen=HashSet::new();
  if req.rows.iter().any(|row|!seen.insert(*row)){return Err(error(StatusCode::BAD_REQUEST,"重复选择了相同行"))}
  let mut rules=Vec::new();
  for row in &req.rows {let r=preview.rules.iter().find(|r|r.row==*row).ok_or_else(||error(StatusCode::BAD_REQUEST,"所选行不属于已解析的 Excel 规则"))?;
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
 let total=rules.len();
 emit_update(&updates,serde_json::json!({"type":"started","total":total})).await;
 let mut results=ImportResult{created:0,skipped:0,failed:0,items:vec![]};
 for (index,r) in rules.iter().enumerate(){
  emit_update(&updates,serde_json::json!({"type":"current","index":index+1,"row":r.row,"name":r.name})).await;
  let item=if names.contains(&r.name){
   ResultRow{row:r.row,name:r.name.clone(),status:"skipped".into(),message:"同业务组内已存在同名规则".into()}
  }else if excel::seconds(&r.duration).is_none()||r.promql.is_empty()||r.name.is_empty()||r.issues.iter().any(|i|i.level=="error"){
   ResultRow{row:r.row,name:r.name.clone(),status:"invalid".into(),message:"规则校验不通过".into()}
  }else{
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
   let outcome=match res{Ok(response)=>api_json(response).await.and_then(|v|creation_result(&v,&r.name)),Err(e)=>Err(e.to_string())};
   match outcome{
    Ok(())=>{
     names.insert(r.name.clone());
     ResultRow{row:r.row,name:r.name.clone(),status:"created".into(),message:"API已接受创建请求（默认禁用）".into()}
    },
    Err(e)=>ResultRow{row:r.row,name:r.name.clone(),status:"failed".into(),message:e}
   }
  };
  match item.status.as_str(){
   "created"=>results.created+=1,
   "skipped"=>results.skipped+=1,
   _=>results.failed+=1
  }
  emit_update(&updates,serde_json::json!({"type":"item","index":index+1,"total":total,"item":item})).await;
  results.items.push(item);
 }
 emit_update(&updates,serde_json::json!({"type":"complete","total":total,"created":results.created,"skipped":results.skipped,"failed":results.failed})).await;
 Ok(results)
}
async fn import(State(state):State<AppState>,Json(req):Json<ImportRequest>)->Result<Json<ImportResult>,(StatusCode,Json<Value>)>{
 import_core(state,req,None).await.map(Json)
}
async fn import_stream(State(state):State<AppState>,Json(req):Json<ImportRequest>)->Response{
 // The import continues server-side even if a browser loses its streaming connection.
 let (tx,rx)=tokio::sync::mpsc::channel::<Bytes>(32);
 tokio::spawn(async move{
  let result=import_core(state,req,Some(tx.clone())).await;
  if let Err((status,Json(details)))=result{
   let message=details.get("error").and_then(Value::as_str).unwrap_or("导入中止");
   emit_update(&Some(tx),serde_json::json!({"type":"error","status":status.as_u16(),"message":message})).await;
  }
 });
 let stream=ReceiverStream::new(rx).map(Ok::<Bytes,std::convert::Infallible>);
 Response::builder()
  .header(header::CONTENT_TYPE,"application/x-ndjson; charset=utf-8")
  .header(header::CACHE_CONTROL,"no-cache, no-store")
  .header("x-accel-buffering","no")
  .body(Body::from_stream(stream))
  .expect("valid response headers")
}
#[tokio::main]async fn main(){
 tracing_subscriber::fmt::init();
 let state:AppState=Arc::new(Mutex::new(HashMap::new()));
 let app=Router::new().route("/",get(index)).route("/api/preview",post(preview)).route("/api/test-connection",post(test_connection)).route("/api/discover",post(discover)).route("/api/import",post(import)).route("/api/import-stream",post(import_stream)).layer(DefaultBodyLimit::max(16*1024*1024)).with_state(state);
 let bind=std::env::var("BIND").unwrap_or("127.0.0.1:8088".into());
 let addr:SocketAddr=bind.parse().expect("BIND must be IP:port");
 let listener=tokio::net::TcpListener::bind(addr).await.unwrap();
 println!("Web http://{addr}");
 axum::serve(listener,app).await.unwrap();
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn nightingale_result_map() {
  assert!(creation_result(&serde_json::json!({"dat":{"example":""},"err":null}),"example").is_ok());
  assert!(creation_result(&serde_json::json!({"dat":{"example":"rule already exists"}}),"example").is_err());
  assert!(creation_result(&serde_json::json!({"dat":{}}),"example").is_err());
 }
 #[test]
 fn existing_rule_list_shapes(){
  assert_eq!(extract_list(&serde_json::json!({"dat":[{"name":"a"}]})).unwrap().len(),1);
  assert_eq!(extract_list(&serde_json::json!({"dat":{"list":[{"name":"a"}],"total":1}})).unwrap().len(),1);
  assert!(extract_list(&serde_json::json!({"dat":null})).is_err());
 }
}


#[cfg(test)]
mod http_integration_tests {
 use super::*;
 use excel::{Issue,Rule};
 #[test]
 fn parses_discovery_items_without_secrets(){
  let input=serde_json::json!({"dat":[{"id":8,"name":"vm-prod","plugin_type":"prometheus","auth":{"password":"DO-NOT-EXPOSE"}},{"id":9,"name":"test","plugin_type":"mysql"}],"err":null});
  let items=parse_options(&input,"datasource").unwrap();
  assert_eq!(items.len(),2);
  let serialized=serde_json::to_string(&items).unwrap();
  assert!(!serialized.contains("DO-NOT-EXPOSE"));
  assert_eq!(items[1].kind,"prometheus");
 }
 #[tokio::test]
 async fn discovers_groups_and_datasources(){
  let mock=Router::new()
   .route("/api/n9e/busi-groups",get(||async{Json(serde_json::json!({"dat":[{"id":2,"name":"Linux生产"}],"err":null}))}))
   .route("/api/n9e/datasource/list",post(|Json(body):Json<Value>|async move{
      assert_eq!(body["plugin_type"],"");
      Json(serde_json::json!({"dat":[{"id":7,"name":"VictoriaMetrics","plugin_type":"prometheus","auth":{"password":"secret"}}],"err":null}))
   }));
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let addr=listener.local_addr().unwrap();
  tokio::spawn(async move{axum::serve(listener,mock).await.unwrap()});
  let result=discover(Json(DiscoverRequest{url:format!("http://{addr}"),token:"test".into()})).await.unwrap().0;
  assert_eq!(result.groups[0].id,2);
  assert_eq!(result.datasources[0].id,7);
  assert_eq!(result.datasources[0].kind,"prometheus");
 }
 #[tokio::test]
 async fn test_connection_validates_accessible_group() {
  let mock=Router::new().route("/api/n9e/busi-group/2/alert-rules",
   get(|headers:axum::http::HeaderMap| async move {
    assert_eq!(headers.get("X-User-Token").and_then(|v|v.to_str().ok()),Some("test-token"));
    Json(serde_json::json!({"err":null,"dat":[{"name":"rule-a"},{"name":"rule-b"}]}))
   }));
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let addr=listener.local_addr().unwrap();
  tokio::spawn(async move{axum::serve(listener,mock).await.unwrap()});
  let request=TestConnectionRequest{
    url:format!("http://{addr}"),token:"test-token".into(),group_id:2,datasource_id:7
  };
  let result=test_connection(Json(request)).await.unwrap().0;
  assert!(result.connected);
  assert_eq!(result.existing_rules,2);
  assert_eq!(result.group_id,2);
  assert!(!result.datasource_checked);
 }
 #[tokio::test]
 async fn test_connection_rejects_invalid_token() {
  let mock=Router::new().route("/api/n9e/busi-group/2/alert-rules",
   get(||async{Json(serde_json::json!({"err":"unauthorized","dat":null}))}));
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let addr=listener.local_addr().unwrap();
  tokio::spawn(async move{axum::serve(listener,mock).await.unwrap()});
  let request=TestConnectionRequest{
    url:format!("http://{addr}"),token:"wrong".into(),group_id:2,datasource_id:7
  };
  assert!(test_connection(Json(request)).await.is_err());
 }
 #[tokio::test]
 async fn import_uses_array_payload_disables_rules_and_skips_existing() {
  let mock=Router::new().route("/api/n9e/busi-group/1/alert-rules",
   get(||async{Json(serde_json::json!({"dat":[{"name":"already"}],"err":null}))})
   .post(|Json(body):Json<Value>|async move{
    assert!(body.is_array(),"Nightingale v8 create API accepts an array");
    let v=&body[0];
    assert_eq!(v["name"],"new");
    assert_eq!(v["disabled"],1);
    assert_eq!(v["prom_for_duration"],0);
    assert_eq!(v["datasource_queries"][0]["values"][0],3);
    assert_eq!(v["rule_config"]["queries"][0]["prom_ql"],"vector(1)>0");
    Json(serde_json::json!({"dat":{"new":""},"err":null}))
   }));
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let addr=listener.local_addr().unwrap();
  tokio::spawn(async move{axum::serve(listener,mock).await.unwrap()});
  let build_rule=|row:u32,name:&str|Rule{
   row,name:name.into(),group:"synthetic".into(),exporter:"test".into(),category:"test".into(),
   promql:"vector(1)>0".into(),suggested_promql:String::new(),
   duration:"0m".into(),duration_seconds:0,suggested_duration:"1m".into(),
   summary:"test".into(),description:String::new(),suggestion:String::new(),
   note:String::new(),notification:String::new(),label_requirements:String::new(),
   recovery:String::new(),repeat:String::new(),issues:Vec::<Issue>::new()
  };
  let state:AppState=Arc::new(Mutex::new(HashMap::new()));
  state.lock().unwrap().insert("fixture".into(),Preview{
   preview_id:"fixture".into(),sheets:vec![],selected:2,all_rules:2,
   rules:vec![build_rule(5,"already"),build_rule(6,"new")],workbook_warnings:vec![]
  });
  let request=ImportRequest{
   url:format!("http://{addr}"),token:"test-token".into(),group_id:1,datasource_id:3,
   preview_id:"fixture".into(),rows:vec![5,6],confirm:true
  };
  let result=import(State(state),Json(request)).await.unwrap().0;
  assert_eq!(result.created,1);
  assert_eq!(result.skipped,1);
  assert_eq!(result.failed,0);
 }
}
