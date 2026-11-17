mod excel;
use axum::{body::{Body,Bytes},extract::{Multipart,DefaultBodyLimit,State},Extension,http::{StatusCode,header},response::{Html,Response},routing::{get,post},Json,Router};
use serde::{Deserialize,Serialize};
use serde_json::Value;
use std::{collections::{HashSet,HashMap},net::SocketAddr,time::{Duration,Instant},sync::{Arc,Mutex}};
use excel::Preview;
use tokio_stream::{wrappers::ReceiverStream,StreamExt};
type AppState=Arc<Mutex<HashMap<String,Preview>>>;
type OverwriteState=Arc<Mutex<HashMap<String,OverwritePlan>>>;
type OverwriteWriteLock=Arc<tokio::sync::Mutex<()>>;
struct OverwritePlan {
 created:Instant, group_id:u64, datasource_id:u64, row:u32, preview_id:String,
 url:String, rule_id:u64, name:String, original:Value, updated:Value,
}
#[derive(Deserialize)]struct OverwritePrepareRequest {
 url:String,token:String,group_id:u64,datasource_id:u64,preview_id:String,row:u32,
 #[serde(default)]parameters:Option<RuleParams>
}
#[derive(Deserialize)]struct OverwriteExecuteRequest {
 url:String,token:String,group_id:u64,datasource_id:u64,preview_id:String,row:u32,
 ticket:String,expected_rule_id:u64,typed_name:String,confirm_overwrite:bool
}
#[derive(Serialize)]struct OverwritePrepareResult {
 ticket:String,rule_id:u64,name:String,expires_seconds:u64,
 changes:Vec<OverwriteChange>,preserved:Vec<String>
}
#[derive(Serialize)]struct OverwriteChange {field:String,before:String,after:String}


#[derive(Clone,Deserialize,Default)]
struct RuleParams {
 recover_duration:Option<u64>,
 notify_repeat_step:Option<u64>,
 notify_max_number:Option<u64>,
 label_requirements:Option<String>,
 #[serde(default)] append_tags:Vec<String>,
}
#[derive(Clone,Deserialize)]struct ImportRequest {
 url:String,token:String,group_id:u64,datasource_id:u64,preview_id:String,rows:Vec<u32>,confirm:bool,
 #[serde(default)] parameters:HashMap<u32,RuleParams>
}
fn validate_append_tag(tag:&str)->bool{
 let Some((key,value))=tag.split_once('=') else {return false};
 !key.is_empty()&&!value.is_empty()&&key.len()<=64&&value.len()<=128
 &&key.chars().enumerate().all(|(i,c)|c.is_ascii_alphanumeric()||c=='_'||(i>0&&c=='.'))
 &&value.chars().all(|c|c.is_ascii_alphanumeric()||"_-.:/".contains(c))
}
fn checked_params(rule:&excel::Rule,override_value:Option<&RuleParams>)->Result<(u64,u64,u64,Vec<String>),String>{
 let recovery=override_value.and_then(|v|v.recover_duration).unwrap_or(rule.recovery_seconds);
 let repeat=override_value.and_then(|v|v.notify_repeat_step).unwrap_or(rule.repeat_minutes);
 let count=override_value.and_then(|v|v.notify_max_number).unwrap_or(rule.max_repeat_count);
 if recovery>86400{return Err("留观时长超过24小时限制".into())}
 if repeat>10080{return Err("重复通知间隔超过7天限制".into())}
 if count>10000{return Err("最大重复次数超过10000限制".into())}
 let tags=override_value.map(|x|x.append_tags.clone()).unwrap_or_default();
 if tags.len()>32||tags.iter().any(|t|!validate_append_tag(t)){return Err("附加标签格式不正确：仅允许32个以内的静态 key=value；不允许动态模板".into())}
 if let Some(text)=override_value.and_then(|x|x.label_requirements.as_ref()){if text.chars().count()>4000{return Err("采集侧标签要求文本过长".into())}}
 Ok((recovery,repeat,count,tags))
}
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
#[derive(Clone,Serialize)]struct ResultRow{row:u32,name:String,status:String,message:String,#[serde(skip_serializing_if="Vec::is_empty")]tags_requested:Vec<String>,#[serde(skip_serializing_if="Option::is_none")]tags_verification:Option<String>}
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

fn observed_append_tags(rule:&Value)->Option<Vec<String>>{
 let v=rule.get("append_tags").or_else(||rule.get("append_tags_json"))?;
 if let Some(a)=v.as_array(){
  return Some(a.iter().filter_map(Value::as_str).map(str::to_owned).collect());
 }
 if let Some(txt)=v.as_str(){
  return Some(txt.split_whitespace().map(str::to_owned).collect());
 }
 None
}
async fn check_created_tags(client:&reqwest::Client,uri:&str,token:&str,name:&str,tags:&[String])->(String,String){
 let response=match client.get(uri).header("X-User-Token",token).send().await{
  Ok(response)=>response,
  Err(e)=>return ("unverified".into(),format!("创建请求已被接受，但回读失败：{e}"))
 };
 let value=match api_json(response).await{
  Ok(value)=>value,
  Err(e)=>return ("unverified".into(),format!("创建请求已被接受，但回读接口异常：{e}"))
 };
 let list=match extract_list(&value){
  Ok(list)=>list,
  Err(e)=>return ("unverified".into(),format!("创建请求已被接受，但无法读取规则列表：{e}"))
 };
 let Some(saved)=list.iter().find(|x|x.get("name").and_then(Value::as_str)==Some(name))else{
  return ("unverified".into(),"创建请求已被接受，但回读列表未找到该规则；可能存在同步延迟".into())
 };
 let Some(actual)=observed_append_tags(saved)else{
  return ("unverified".into(),"创建请求已被接受，但回读列表未包含 append_tags 字段；无法确认标签是否持久化".into())
 };
 let expected:HashSet<&str>=tags.iter().map(String::as_str).collect();
 let found:HashSet<&str>=actual.iter().map(String::as_str).collect();
 if expected.is_subset(&found){
  ("verified".into(),format!("附加标签回读验证成功（{} 个）",tags.len()))
 }else{
  let missing:Vec<&str>=expected.difference(&found).copied().collect();
  ("mismatch".into(),format!("创建 API 已接受，但回读未发现标签：{}；请检查 Nightingale 规则详情",missing.join(", ")))
 }
}

fn checked_rule_from_preview(state:&AppState,preview_id:&str,row:u32)->Result<excel::Rule,(StatusCode,Json<Value>)>{
 let cache=state.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"预览状态锁异常"))?;
 let preview=cache.get(preview_id).ok_or_else(||error(StatusCode::BAD_REQUEST,"预览已过期，请重新上传文件"))?;
 let rule=preview.rules.iter().find(|r|r.row==row).ok_or_else(||error(StatusCode::BAD_REQUEST,"规则不在当前Excel预览中"))?;
 if rule.name.trim().is_empty()||rule.promql.trim().is_empty()||rule.issues.iter().any(|i|i.level=="error"){
  return Err(error(StatusCode::BAD_REQUEST,"规则未通过校验，禁止覆盖"))
 }
 Ok(rule.clone())
}
fn alert_identity(rule:&Value,id:u64,name:&str,group_id:u64)->bool{
 rule.get("id").and_then(Value::as_u64)==Some(id)
 &&rule.get("name").and_then(Value::as_str)==Some(name)
 &&rule.get("group_id").and_then(Value::as_u64)==Some(group_id)
 &&rule.get("cate").and_then(Value::as_str)==Some("prometheus")
 &&rule.get("prod").and_then(Value::as_str).map(|p|p=="metric"||p=="").unwrap_or(true)
}
fn overwrite_target_disabled(rule:&Value)->bool{
 // Strictly require the numeric 1, not a missing, null, string, or bool value.
 rule.get("disabled").and_then(Value::as_u64)==Some(1)
}
fn make_overwrite_payload(original:&Value,rule:&excel::Rule,group_id:u64,datasource_id:u64,params:Option<&RuleParams>)->Result<Value,String>{
 if !overwrite_target_disabled(original){return Err("目标规则必须为 Disabled（disabled=1）；请先在 Nightingale 停用该规则".into())}
 let (recovery,repeat,count,tags)=checked_params(rule,params)?;
 let mut updated=original.clone();
 let obj=updated.as_object_mut().ok_or("夜莺原规则不是JSON对象")?;
 let cfg=obj.get("rule_config").and_then(Value::as_object).ok_or("规则配置缺失或格式无法识别，拒绝覆盖")?;
 if cfg.get("version").and_then(Value::as_str)==Some("v2") {return Err("当前规则为 Prometheus v2，暂不支持覆盖".into())}
 let queries=cfg.get("queries").and_then(Value::as_array).ok_or("原规则缺少查询数组")?;
 if queries.len()!=1{return Err("只支持单查询规则；多查询规则禁止覆盖".into())}
 let query=queries[0].as_object().ok_or("原查询配置无效")?;
 if !query.contains_key("prom_ql"){return Err("原查询不是 PromQL 规则".into())}
 let mut config=obj.get("rule_config").cloned().unwrap();
 let query=config.get_mut("queries").and_then(Value::as_array_mut).and_then(|a|a.get_mut(0)).and_then(Value::as_object_mut).ok_or("原查询配置无法修改")?;
 query.insert("prom_ql".into(),Value::String(rule.promql.clone()));
 query.insert("severity".into(),serde_json::json!(1));
 obj.insert("rule_config".into(),config);
 // Nightingale FE2DB prioritizes legacy prom_ql when non-empty and rebuilds rule_config.
 // Clear the deprecated shadow field so the reviewed single-query rule_config survives.
 obj.insert("prom_ql".into(),Value::String(String::new()));
 obj.insert("note".into(),Value::String(rule.note.clone()));
 obj.insert("prom_for_duration".into(),serde_json::json!(rule.duration_seconds));
 obj.insert("prom_eval_interval".into(),serde_json::json!(30));
 obj.insert("recover_duration".into(),serde_json::json!(recovery));
 obj.insert("notify_repeat_step".into(),serde_json::json!(repeat));
 obj.insert("notify_max_number".into(),serde_json::json!(count));
 obj.insert("datasource_queries".into(),serde_json::json!([{"match_type":0,"op":"in","values":[datasource_id]}]));
 obj.insert("append_tags".into(),serde_json::json!(tags));
 let annotations=obj.entry("annotations".to_string()).or_insert_with(||serde_json::json!({}));
 let map=annotations.as_object_mut().ok_or("原注释信息格式异常")?;
 map.insert("summary".into(),Value::String(rule.summary.clone()));
 map.insert("description".into(),Value::String(rule.description.clone()));
 map.insert("suggestion".into(),Value::String(rule.suggestion.clone()));
 // Never touch disabled, notify rule IDs, schedule or callbacks.
 if obj.get("group_id").and_then(Value::as_u64)!=Some(group_id){return Err("原规则业务组不匹配".into())}
 Ok(updated)
}
fn overwrite_changes(original:&Value,updated:&Value)->Vec<OverwriteChange>{
 const FIELDS:&[&str]=&["rule_config","prom_ql","note","prom_for_duration","prom_eval_interval","recover_duration","notify_repeat_step","notify_max_number","datasource_queries","append_tags","annotations"];
 FIELDS.iter().filter_map(|key|{
  let a=original.get(key); let b=updated.get(key);
  if a==b{return None}
  Some(OverwriteChange{
   field:key.to_string(),
   before:a.and_then(|v|serde_json::to_string(v).ok()).unwrap_or_else(||"未设置".into()),
   after:b.and_then(|v|serde_json::to_string(v).ok()).unwrap_or_else(||"未设置".into())
  })
 }).collect()
}
async fn fetch_rule_detail(client:&reqwest::Client,base:&str,token:&str,id:u64)->Result<Value,(StatusCode,Json<Value>)>{
 let response=client.get(format!("{base}/api/n9e/alert-rule/{id}/pure"))
  .header("X-User-Token",token).send().await.map_err(|e|error(StatusCode::BAD_GATEWAY,format!("读取现有规则失败：{e}")))?;
 let value=api_json(response).await.map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 value.get("dat").or_else(||value.get("data")).filter(|v|v.is_object())
  .cloned().ok_or_else(||error(StatusCode::BAD_GATEWAY,"无法读取告警规则完整详情"))
}
async fn overwrite_prepare(
 State(state):State<AppState>,Extension(tickets):Extension<OverwriteState>,
 Json(req):Json<OverwritePrepareRequest>
)->Result<Json<OverwritePrepareResult>,(StatusCode,Json<Value>)>{
 if req.token.trim().is_empty()||req.group_id==0||req.datasource_id==0{return Err(error(StatusCode::BAD_REQUEST,"连接信息或业务组/数据源未设置"))}
 let rule=checked_rule_from_preview(&state,&req.preview_id,req.row)?;
 let base=valid_url(&req.url).map_err(|e|error(StatusCode::BAD_REQUEST,e))?;
 let client=reqwest::Client::builder().timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
 let list_url=format!("{base}/api/n9e/busi-group/{}/alert-rules",req.group_id);
 let response=client.get(&list_url).header("X-User-Token",&req.token).send().await.map_err(|e|error(StatusCode::BAD_GATEWAY,e.to_string()))?;
 let value=api_json(response).await.map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 let list=extract_list(&value).map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 let matches:Vec<&Value>=list.iter().filter(|x|x.get("name").and_then(Value::as_str)==Some(rule.name.as_str())).collect();
 if matches.len()!=1{return Err(error(StatusCode::CONFLICT,format!("同名规则匹配到 {} 条，必须恰好一条才允许覆盖",matches.len())))}
 let id=matches[0].get("id").and_then(Value::as_u64).filter(|x|*x>0).ok_or_else(||error(StatusCode::BAD_GATEWAY,"目标规则缺少有效ID"))?;
 let current=fetch_rule_detail(&client,&base,&req.token,id).await?;
 if !alert_identity(&current,id,&rule.name,req.group_id){return Err(error(StatusCode::CONFLICT,"现有规则ID、名称或业务组不一致，覆盖已取消"))}
 if !overwrite_target_disabled(&current){return Err(error(StatusCode::CONFLICT,"禁止覆盖：夜莺现有规则必须为 Disabled（disabled=1）。请先在 Nightingale 停用规则，再重新点击覆盖"))}
 let updated=make_overwrite_payload(&current,&rule,req.group_id,req.datasource_id,req.parameters.as_ref()).map_err(|e|error(StatusCode::BAD_REQUEST,e))?;
 let changes=overwrite_changes(&current,&updated);
 if changes.is_empty(){return Err(error(StatusCode::CONFLICT,"现有配置与Excel配置一致，无需覆盖"))}
 let ticket=uuid::Uuid::new_v4().to_string();
 {
  let mut store=tickets.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"确认状态锁异常"))?;
  store.retain(|_,p|p.created.elapsed()<Duration::from_secs(300));
  if store.len()>200{store.clear()}
  store.insert(ticket.clone(),OverwritePlan{
   created:Instant::now(),group_id:req.group_id,datasource_id:req.datasource_id,row:req.row,
   preview_id:req.preview_id,url:base,rule_id:id,name:rule.name.clone(),original:current,updated
  });
 }
 Ok(Json(OverwritePrepareResult{ticket,rule_id:id,name:rule.name.clone(),expires_seconds:300,changes,
  preserved:vec!["原规则启用/禁用状态".into(),"通知关联、回调与生效时间段".into(),"其他Excel未管理的夜莺配置".into()]}))
}
async fn overwrite_execute(
 Extension(tickets):Extension<OverwriteState>,Extension(write_lock):Extension<OverwriteWriteLock>,Json(req):Json<OverwriteExecuteRequest>
)->Result<Json<Value>,(StatusCode,Json<Value>)>{
 if !req.confirm_overwrite||req.token.trim().is_empty(){return Err(error(StatusCode::BAD_REQUEST,"覆盖必须完成最终确认"))}
 // Remove BEFORE network operations: a ticket can never be replayed, even if an update fails.
 let plan=tickets.lock().map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"确认状态锁异常"))?
  .remove(&req.ticket).ok_or_else(||error(StatusCode::CONFLICT,"确认凭据无效或已使用，请重新检查差异"))?;
 if plan.created.elapsed()>=Duration::from_secs(300){return Err(error(StatusCode::CONFLICT,"确认已过期，请重新预览"))}
 let base=valid_url(&req.url).map_err(|e|error(StatusCode::BAD_REQUEST,e))?;
 if plan.url!=base||plan.group_id!=req.group_id||plan.datasource_id!=req.datasource_id||
  plan.preview_id!=req.preview_id||plan.row!=req.row||plan.rule_id!=req.expected_rule_id||
  req.typed_name!=plan.name
 {return Err(error(StatusCode::CONFLICT,"覆盖目标与确认预览不一致，操作已取消"))}
 let _write_guard=write_lock.lock().await;
 let client=reqwest::Client::builder().timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().map_err(|e|error(StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
 let latest=fetch_rule_detail(&client,&base,&req.token,plan.rule_id).await?;
 if latest!=plan.original{return Err(error(StatusCode::CONFLICT,"夜莺规则自预览后发生变化，禁止覆盖；请重新查看差异"))}
 if !alert_identity(&latest,plan.rule_id,&plan.name,plan.group_id){return Err(error(StatusCode::CONFLICT,"规则身份检查失败"))}
 if !overwrite_target_disabled(&latest){return Err(error(StatusCode::CONFLICT,"禁止覆盖：目标规则目前不是 Disabled（disabled=1），请先在 Nightingale 停用并重新确认"))}
 let uri=format!("{base}/api/n9e/busi-group/{}/alert-rule/{}",plan.group_id,plan.rule_id);
 let response=client.put(&uri).header("X-User-Token",&req.token).json(&plan.updated).send().await.map_err(|e|error(StatusCode::BAD_GATEWAY,format!("覆盖请求失败：{e}")))?;
 api_json(response).await.map_err(|e|error(StatusCode::BAD_GATEWAY,e))?;
 let saved=fetch_rule_detail(&client,&base,&req.token,plan.rule_id).await?;
 let mut mismatch=Vec::new();
 for field in ["rule_config","prom_for_duration","recover_duration","notify_repeat_step","notify_max_number","append_tags","datasource_queries"]{
  if saved.get(field)!=plan.updated.get(field){mismatch.push(field)}
 }
 Ok(Json(serde_json::json!({"rule_id":plan.rule_id,"name":plan.name,
  "updated":true,"verified":mismatch.is_empty(),"mismatch_fields":mismatch,
  "message":if mismatch.is_empty(){"单条规则覆盖成功，关键字段回读一致"}else{"更新API已接受，但有字段回读不一致，请检查夜莺实际配置"}})))
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
 for r in &rules {
  checked_params(r,req.parameters.get(&r.row)).map_err(|e|error(StatusCode::BAD_REQUEST,format!("规则 {} 参数错误：{e}",r.name)))?;
 }
 let total=rules.len();
 emit_update(&updates,serde_json::json!({"type":"started","total":total})).await;
 let mut results=ImportResult{created:0,skipped:0,failed:0,items:vec![]};
 for (index,r) in rules.iter().enumerate(){
  emit_update(&updates,serde_json::json!({"type":"current","index":index+1,"row":r.row,"name":r.name})).await;
  let item=if names.contains(&r.name){
   ResultRow{row:r.row,name:r.name.clone(),status:"skipped".into(),message:"同业务组内已存在同名规则；未修改已有规则及附加标签".into(),tags_requested:Vec::new(),tags_verification:None}
  }else if excel::seconds(&r.duration).is_none()||r.promql.is_empty()||r.name.is_empty()||r.issues.iter().any(|i|i.level=="error"){
   ResultRow{row:r.row,name:r.name.clone(),status:"invalid".into(),message:"规则校验不通过".into(),tags_requested:Vec::new(),tags_verification:None}
  }else{
   let (recovery,repeat,count,tags)=checked_params(r,req.parameters.get(&r.row)).expect("validated before import");
   let payload=serde_json::json!({
    "name":r.name,"note":r.note,
    "group_id":req.group_id,"datasource_queries":[{"match_type":0,"op":"in","values":[req.datasource_id]}],
    "cate":"prometheus","prod":"metric","severity":1,"disabled":1,
    "prom_for_duration":r.duration_seconds,"prom_eval_interval":30,"prom_ql":"",
    "enable_stimes":["00:00"],"enable_etimes":["00:00"],
    "enable_days_of_weeks":[["0","1","2","3","4","5","6"]],
    "annotations":{"summary":r.summary,"description":r.description,"suggestion":r.suggestion},
    "append_tags":tags,"notify_rule_ids":[],
    "rule_config":{"queries":[{"prom_ql":r.promql,"severity":1}]},
    "notify_repeat_step":repeat,"notify_max_number":count,"recover_duration":recovery
   });
   let res=client.post(&uri).header("X-User-Token",&req.token).json(&vec![payload]).send().await;
   let outcome=match res{Ok(response)=>api_json(response).await.and_then(|v|creation_result(&v,&r.name)),Err(e)=>Err(e.to_string())};
   match outcome{
    Ok(())=>{
     names.insert(r.name.clone());
     {
     if tags.is_empty(){
      ResultRow{row:r.row,name:r.name.clone(),status:"created".into(),message:"API已接受创建请求（默认禁用）；未配置静态附加标签".into(),tags_requested:tags,tags_verification:None}
     }else{
      let (verification,detail)=check_created_tags(&client,&uri,&req.token,&r.name,&tags).await;
      ResultRow{row:r.row,name:r.name.clone(),status:"created".into(),
       message:format!("API已接受创建请求（默认禁用）；{detail}"),
       tags_requested:tags,tags_verification:Some(verification)}
     }
    }
    },
    Err(e)=>ResultRow{row:r.row,name:r.name.clone(),status:"failed".into(),message:e,tags_requested:tags,tags_verification:None}
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
 let overwrite_state:OverwriteState=Arc::new(Mutex::new(HashMap::new()));
 let overwrite_write_lock:OverwriteWriteLock=Arc::new(tokio::sync::Mutex::new(()));
 let app=Router::new().route("/",get(index)).route("/api/preview",post(preview)).route("/api/test-connection",post(test_connection)).route("/api/discover",post(discover)).route("/api/import",post(import)).route("/api/import-stream",post(import_stream)).route("/api/overwrite/prepare",post(overwrite_prepare)).route("/api/overwrite/execute",post(overwrite_execute)).layer(DefaultBodyLimit::max(16*1024*1024)).with_state(state).layer(Extension(overwrite_state)).layer(Extension(overwrite_write_lock));
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
 fn recognizes_readback_append_tags(){
  let rule=serde_json::json!({"name":"test","append_tags":["team=ops","env=prod"]});
  assert_eq!(observed_append_tags(&rule).unwrap(),vec!["team=ops","env=prod"]);
  let alt=serde_json::json!({"append_tags_json":"team=ops env=prod"});
  assert_eq!(observed_append_tags(&alt).unwrap(),vec!["team=ops","env=prod"]);
  assert_eq!(observed_append_tags(&serde_json::json!({"name":"old"})),None);
 }
 #[test]
 fn validates_rule_static_tags(){
  assert!(validate_append_tag("team=ops"));
  assert!(!validate_append_tag("host={{主机名}}"));
  assert!(!validate_append_tag("env=prod;foo=bar"));
  assert!(!validate_append_tag("bad key=x"));
 }
 #[test]
 fn validates_rule_parameter_overrides(){
  let rule=excel::Rule{
   row:5,name:"rule".into(),group:String::new(),exporter:String::new(),category:String::new(),
   promql:"up == 0".into(),suggested_promql:String::new(),duration:"1m".into(),duration_seconds:60,
   suggested_duration:String::new(),summary:String::new(),description:String::new(),suggestion:String::new(),
   note:String::new(),notification:String::new(),label_requirements:"host={{主机名}}".into(),
   recovery:"5 分钟".into(),repeat:"60 分钟".into(),max_repeat:"0（不限）".into(),
   recovery_seconds:300,repeat_minutes:60,max_repeat_count:0,issues:vec![]
  };
  let overrides=RuleParams{recover_duration:Some(600),notify_repeat_step:Some(45),
   notify_max_number:Some(3),label_requirements:None,append_tags:vec!["team=ops".into()]};
  let (r,n,m,tags)=checked_params(&rule,Some(&overrides)).unwrap();
  assert_eq!((r,n,m),(600,45,3));
  assert_eq!(tags,vec!["team=ops"]);
  let invalid=RuleParams{append_tags:vec!["host={{主机名}}".into()],..Default::default()};
  assert!(checked_params(&rule,Some(&invalid)).is_err());
 }
 #[test]
 fn force_overwrite_requires_existing_rule_disabled(){
  let mut rule=serde_json::json!({"id":88,"name":"test","group_id":5,"cate":"prometheus","prod":"metric","disabled":1});
  assert!(overwrite_target_disabled(&rule));
  rule["disabled"]=serde_json::json!(0);
  assert!(!overwrite_target_disabled(&rule));
  rule["disabled"]=serde_json::json!("1");
  assert!(!overwrite_target_disabled(&rule));
  rule.as_object_mut().unwrap().remove("disabled");
  assert!(!overwrite_target_disabled(&rule));
 }
 #[test]
 fn overwrite_payload_preserves_runtime_state_and_only_one_query(){
  let rule=excel::Rule{
   row:5,name:"CPU High".into(),group:String::new(),exporter:String::new(),category:String::new(),
   promql:"up == 0".into(),suggested_promql:String::new(),duration:"2m".into(),duration_seconds:120,
   suggested_duration:String::new(),summary:"new summary".into(),description:"new description".into(),
   suggestion:String::new(),note:"new note".into(),notification:String::new(),
   label_requirements:String::new(),recovery:"0m".into(),repeat:"10m".into(),
   max_repeat:"2".into(),recovery_seconds:0,repeat_minutes:10,max_repeat_count:2,issues:vec![]
  };
  let original=serde_json::json!({"id":19,"name":"CPU High","group_id":5,
   "cate":"prometheus","prod":"metric","disabled":1,"notify_rule_ids":[99],
   "callbacks":["https://example.net"],"enable_stimes":["08:00"],
   "rule_config":{"queries":[{"prom_ql":"up > 0","severity":2}]},
   "append_tags":["original=tag"],"annotations":{"owner":"keep","summary":"old"}});
  let params=RuleParams{append_tags:vec!["team=ops".into()],..Default::default()};
  let updated=make_overwrite_payload(&original,&rule,5,7,Some(&params)).unwrap();
  assert!(alert_identity(&original,19,"CPU High",5));
  assert!(!alert_identity(&original,18,"CPU High",5));
  assert_eq!(updated["disabled"],1);
  assert_eq!(updated["notify_rule_ids"],serde_json::json!([99]));
  assert_eq!(updated["callbacks"],serde_json::json!(["https://example.net"]));
  assert_eq!(updated["enable_stimes"],serde_json::json!(["08:00"]));
  assert_eq!(updated["rule_config"]["queries"][0]["prom_ql"],"up == 0");
  assert_eq!(updated["prom_ql"],"");
  assert_eq!(updated["rule_config"]["queries"][0]["severity"],1);
  assert_eq!(updated["append_tags"],serde_json::json!(["team=ops"]));
  assert_eq!(updated["annotations"]["owner"],"keep");
  assert_eq!(updated["annotations"]["summary"],"new summary");
  let mut multiple=original.clone();
  multiple["rule_config"]["queries"]=serde_json::json!([{"prom_ql":"up","severity":1},{"prom_ql":"down","severity":1}]);
  assert!(make_overwrite_payload(&multiple,&rule,5,7,Some(&params)).is_err());
  let mut enabled=original.clone();
  enabled["disabled"]=serde_json::json!(0);
  assert!(make_overwrite_payload(&enabled,&rule,5,7,Some(&params)).is_err());
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
 async fn overwrite_flow_is_one_rule_one_time_and_preserves_enabled_state(){
  use std::sync::atomic::{AtomicUsize,Ordering};
  let current=Arc::new(Mutex::new(serde_json::json!({
   "id":98,"name":"CPU High","group_id":5,"cate":"prometheus","prod":"metric",
   "disabled":1,"notify_rule_ids":[13],"append_tags":["team=old"],
   "rule_config":{"queries":[{"prom_ql":"up == 1","severity":1}]},
   "annotations":{"summary":"old"},"prom_for_duration":60,
   "prom_eval_interval":30,"recover_duration":0,"notify_repeat_step":0,"notify_max_number":0,
   "datasource_queries":[{"match_type":0,"op":"in","values":[7]}]
  })));
  let writes=Arc::new(AtomicUsize::new(0));
  let mock=Router::new()
   .route("/api/n9e/busi-group/5/alert-rules",get({
     move ||async {Json(serde_json::json!({"err":null,"dat":[{"id":98,"name":"CPU High"}]}))}
   }))
   .route("/api/n9e/alert-rule/98/pure",get({
     let current=current.clone();
     move ||{let current=current.clone();async move{Json(serde_json::json!({"err":null,"dat":current.lock().unwrap().clone()}))}}
   }))
   .route("/api/n9e/busi-group/5/alert-rule/98",axum::routing::put({
     let current=current.clone();let writes=writes.clone();
     move |Json(body):Json<Value>|{
      let current=current.clone();let writes=writes.clone();
      async move{writes.fetch_add(1,Ordering::SeqCst);*current.lock().unwrap()=body;Json(serde_json::json!({"err":null}))}
     }
   }));
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let addr=listener.local_addr().unwrap();
  tokio::spawn(async move{axum::serve(listener,mock).await.unwrap()});
  let base=format!("http://{addr}");
  let rule=Rule{row:5,name:"CPU High".into(),group:String::new(),exporter:String::new(),category:String::new(),
   promql:"up == 0".into(),suggested_promql:String::new(),duration:"2m".into(),duration_seconds:120,
   suggested_duration:String::new(),summary:"updated".into(),description:String::new(),suggestion:String::new(),
   note:String::new(),notification:String::new(),label_requirements:String::new(),
   recovery:"0m".into(),repeat:"10m".into(),max_repeat:"2".into(),
   recovery_seconds:0,repeat_minutes:10,max_repeat_count:2,issues:Vec::<Issue>::new()};
  let state:AppState=Arc::new(Mutex::new(HashMap::new()));
  state.lock().unwrap().insert("fixture".into(),Preview{
   preview_id:"fixture".into(),sheets:vec![],selected:1,all_rules:1,
   rules:vec![rule],workbook_warnings:vec![]});
  let tickets:OverwriteState=Arc::new(Mutex::new(HashMap::new()));
  let lock:OverwriteWriteLock=Arc::new(tokio::sync::Mutex::new(()));
  let mk_prepare=||OverwritePrepareRequest{
   url:base.clone(),token:"token".into(),group_id:5,datasource_id:7,
   preview_id:"fixture".into(),row:5,
   parameters:Some(RuleParams{append_tags:vec!["team=ops".into()],..Default::default()})};
  let prepared=overwrite_prepare(State(state.clone()),Extension(tickets.clone()),Json(mk_prepare())).await.unwrap().0;
  assert_eq!(prepared.rule_id,98);
  assert_eq!(writes.load(Ordering::SeqCst),0,"prepare is read-only");
  let mk_execute=|ticket:String,typed_name:&str|OverwriteExecuteRequest{
   url:base.clone(),token:"token".into(),group_id:5,datasource_id:7,preview_id:"fixture".into(),
   row:5,ticket,expected_rule_id:98,typed_name:typed_name.into(),confirm_overwrite:true};
  let rejected=overwrite_execute(Extension(tickets.clone()),Extension(lock.clone()),
   Json(mk_execute(prepared.ticket.clone(),"wrong name"))).await;
  assert!(rejected.is_err());assert_eq!(writes.load(Ordering::SeqCst),0);
  assert!(overwrite_execute(Extension(tickets.clone()),Extension(lock.clone()),
   Json(mk_execute(prepared.ticket,"CPU High"))).await.is_err(),"ticket must be spent even on rejection");
  let prepared=overwrite_prepare(State(state),Extension(tickets.clone()),Json(mk_prepare())).await.unwrap().0;
  let reply=overwrite_execute(Extension(tickets.clone()),Extension(lock),
   Json(mk_execute(prepared.ticket,"CPU High"))).await.unwrap().0;
  assert_eq!(reply["updated"],true);
  assert_eq!(reply["verified"],true);
  assert_eq!(writes.load(Ordering::SeqCst),1,"one update and no deletes");
  let saved=current.lock().unwrap();
  assert_eq!(saved["disabled"],1);
  assert_eq!(saved["notify_rule_ids"],serde_json::json!([13]));
  assert_eq!(saved["append_tags"],serde_json::json!(["team=ops"]));
  assert_eq!(saved["rule_config"]["queries"][0]["prom_ql"],"up == 0");
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
 async fn verifies_static_tags_from_nightingale_readback(){
  let mock=Router::new().route("/api/n9e/busi-group/1/alert-rules",
   get(||async{Json(serde_json::json!({"dat":[{"name":"created-rule","append_tags":["team=ops","env=prod"]}],"err":null}))}));
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let addr=listener.local_addr().unwrap();
  tokio::spawn(async move{axum::serve(listener,mock).await.unwrap()});
  let client=reqwest::Client::new();
  let uri=format!("http://{addr}/api/n9e/busi-group/1/alert-rules");
  let (kind,message)=check_created_tags(&client,&uri,"test","created-rule",&["team=ops".into()]).await;
  assert_eq!(kind,"verified");
  assert!(message.contains("成功"));
  let (kind,message)=check_created_tags(&client,&uri,"test","created-rule",&["service=missing".into()]).await;
  assert_eq!(kind,"mismatch");
  assert!(message.contains("service=missing"));
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
    assert_eq!(v["recover_duration"],300);
    assert_eq!(v["notify_repeat_step"],60);
    assert_eq!(v["notify_max_number"],0);
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
   recovery:String::new(),repeat:String::new(),max_repeat:String::new(),recovery_seconds:300,repeat_minutes:60,max_repeat_count:0,issues:Vec::<Issue>::new()
  };
  let state:AppState=Arc::new(Mutex::new(HashMap::new()));
  state.lock().unwrap().insert("fixture".into(),Preview{
   preview_id:"fixture".into(),sheets:vec![],selected:2,all_rules:2,
   rules:vec![build_rule(5,"already"),build_rule(6,"new")],workbook_warnings:vec![]
  });
  let request=ImportRequest{
   url:format!("http://{addr}"),token:"test-token".into(),group_id:1,datasource_id:3,
   preview_id:"fixture".into(),rows:vec![5,6],confirm:true,parameters:HashMap::new()
  };
  let result=import(State(state.clone()),Json(request.clone())).await.unwrap().0;
  assert_eq!(result.created,1);
  assert_eq!(result.skipped,1);
  assert_eq!(result.failed,0);
  assert_eq!(result.items[0].row,5);
  assert_eq!(result.items[1].row,6);
  // Streaming and JSON endpoints must share the same import logic.
  let (tx,mut rx)=tokio::sync::mpsc::channel::<Bytes>(16);
  let streamed=import_core(state,request,Some(tx)).await.unwrap();
  assert_eq!(streamed.created,1);
  assert_eq!(streamed.skipped,1);
  let mut events=Vec::<Value>::new();
  while let Some(chunk)=rx.recv().await {
   let event:Value=serde_json::from_slice(chunk.strip_suffix(b"\n").unwrap_or(&chunk)).unwrap();
   events.push(event);
  }
  assert_eq!(events.len(),6); // started, current/item x2, complete
  assert_eq!(events[0]["type"],"started");
  assert_eq!(events[0]["total"],2);
  assert_eq!(events[1]["type"],"current");
  assert_eq!(events[2]["type"],"item");
  assert_eq!(events[2]["item"]["status"],"skipped");
  assert_eq!(events[4]["item"]["row"],6);
  assert_eq!(events[5]["type"],"complete");
  assert_eq!(events[5]["created"],1);
 }
}
