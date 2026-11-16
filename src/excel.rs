//! Parse the supplied 11-sheet XLSX. Resolve foreground fill colors via OOXML styles.
use serde::{Deserialize,Serialize};
use std::{collections::{HashMap,HashSet},io::{Cursor,Read}};
use zip::ZipArchive;

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Issue { pub level:String, pub message:String }
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Rule {
    pub row:u32,pub name:String,pub group:String,pub exporter:String,pub category:String,
    pub promql:String,pub suggested_promql:String,pub duration:String,pub duration_seconds:u64,
    pub suggested_duration:String,pub summary:String,pub description:String,pub suggestion:String,
    pub note:String,pub notification:String,pub label_requirements:String,
    pub recovery:String,pub repeat:String,pub issues:Vec<Issue>,
}
#[derive(Serialize)]
pub struct Preview {pub preview_id:String,pub sheets:Vec<String>,pub selected:usize,pub all_rules:usize,pub rules:Vec<Rule>,pub workbook_warnings:Vec<String>}

type Rows=Vec<HashMap<String,String>>;
fn read_file(zip:&mut ZipArchive<Cursor<&[u8]>>,name:&str)->Result<String,String>{
 let mut f=zip.by_name(name).map_err(|e|format!("缺少 {name}: {e}"))?;
 let mut s=String::new(); f.read_to_string(&mut s).map_err(|e|e.to_string())?;Ok(s)
}
fn text(n:roxmltree::Node<'_,'_>)->String{n.descendants().filter(|x|x.is_element()&&x.tag_name().name()=="t").filter_map(|x|x.text()).collect::<Vec<_>>().join("")}
fn shared_strings(xml:&str)->Result<Vec<String>,String>{let d=roxmltree::Document::parse(xml).map_err(|e|e.to_string())?;Ok(d.descendants().filter(|n|n.is_element()&&n.tag_name().name()=="si").map(text).collect())}
fn is_red(v:&str)->bool{let a=v.trim_start_matches('#').to_ascii_uppercase();let a=if a.len()==8{&a[2..]}else{&a};if a.len()!=6{return false};let r=u8::from_str_radix(&a[0..2],16).unwrap_or(0);let g=u8::from_str_radix(&a[2..4],16).unwrap_or(255);let b=u8::from_str_radix(&a[4..6],16).unwrap_or(255);r>=180&&g<=90&&b<=90}
fn red_styles(xml:&str)->Result<HashSet<usize>,String>{
 let d=roxmltree::Document::parse(xml).map_err(|e|e.to_string())?;
 let fills=d.descendants().find(|n|n.has_tag_name("fills")).ok_or("没有 fills")?;
 let red_fills:HashSet<usize>=fills.children().filter(|x|x.has_tag_name("fill")).enumerate().filter_map(|(i,n)|{
  let red=n.descendants().any(|x|x.has_tag_name("fgColor")&&x.attribute("rgb").map(is_red).unwrap_or(false));red.then_some(i)
 }).collect();
 let xfs=d.descendants().find(|n|n.has_tag_name("cellXfs")).ok_or("没有 cellXfs")?;
 Ok(xfs.children().filter(|x|x.has_tag_name("xf")).enumerate().filter_map(|(i,n)|{
  n.attribute("fillId").and_then(|v|v.parse::<usize>().ok()).filter(|id|red_fills.contains(id)).map(|_|i)
 }).collect())
}
fn parse_rows(xml:&str,strings:&[String])->Result<Rows,String>{
 let d=roxmltree::Document::parse(xml).map_err(|e|e.to_string())?;
 let mut out=Vec::new();
 for r in d.descendants().filter(|n|n.has_tag_name("row")){
  let mut row=HashMap::new();row.insert("#row".into(),r.attribute("r").unwrap_or("0").into());
  for c in r.children().filter(|n|n.has_tag_name("c")){
   let reference=c.attribute("r").unwrap_or("");let col=reference.chars().take_while(|x|x.is_ascii_alphabetic()).collect::<String>();
   let v=c.children().find(|n|n.has_tag_name("v")).and_then(|x|x.text()).unwrap_or("");
   let value=match c.attribute("t") {Some("s")=>v.parse::<usize>().ok().and_then(|i|strings.get(i)).cloned().unwrap_or_default(),Some("inlineStr")=>text(c),_=>v.to_owned()};
   if !value.is_empty(){row.insert(col,value);}
   if col=="A"{row.insert("#style".into(),c.attribute("s").unwrap_or("0").into());}
  }
  out.push(row)
 }Ok(out)
}
fn get<'a>(r:&'a HashMap<String,String>,column:&str)->&'a str{r.get(column).map(|s|s.as_str()).unwrap_or("")}
fn index_by_name(rows:&Rows,col:&str)->HashMap<String,HashMap<String,String>>{rows.iter().filter(|r|r.get("#row").and_then(|v|v.parse::<u32>().ok()).unwrap_or(0)>=5).filter(|r|!get(r,col).is_empty()).map(|r|(get(r,col).trim().to_string(),r.clone())).collect()}
pub fn seconds(v:&str)->Option<u64>{let s=v.trim();if s.is_empty(){return None};
 if let Some(a)=s.strip_suffix("ms"){return a.trim().parse::<u64>().ok().map(|x|x/1000)}
 for (suffix,factor) in [("分钟",60),("小时",3600),("秒",1),("m",60),("h",3600),("s",1),("d",86400)]{
  if let Some(a)=s.strip_suffix(suffix){return a.trim().parse::<u64>().ok().map(|x|x*factor)}
 }s.parse::<u64>().ok()
}
fn issue(level:&str,message:impl Into<String>)->Issue{Issue{level:level.into(),message:message.into()}}
pub fn parse(data:&[u8])->Result<Preview,String>{
 if data.len()>16*1024*1024{return Err("文件超过16MB限制".into())}
 let mut zip=ZipArchive::new(Cursor::new(data)).map_err(|e|format!("不是有效xlsx：{e}"))?;
 let ss=if zip.file_names().any(|n|n=="xl/sharedStrings.xml"){shared_strings(&read_file(&mut zip,"xl/sharedStrings.xml")?)?}else{vec![]};
 let xml=read_file(&mut zip,"xl/workbook.xml")?;
 let w=roxmltree::Document::parse(&xml).map_err(|e|e.to_string())?;
 let names:Vec<String>=w.descendants().filter(|n|n.has_tag_name("sheet")).filter_map(|n|n.attribute("name").map(str::to_owned)).collect();
 let expected=["说明","严重告警清单","规则配置明细","告警参数配置","告警参数规范","采集侧标签配置","告警侧派生标签","host_ip 可派生性","通知路由配置","消息模板","待确认事项"];
 if names.len()!=11||expected.iter().any(|s|!names.iter().any(|n|n==s)){return Err("工作簿的11个Sheet与模板不一致，已拒绝导入".into())}
 let reds=red_styles(&read_file(&mut zip,"xl/styles.xml")?)?;
 if reds.is_empty(){return Err("未发现红色填充单元格样式".into())}
 let mut sheets=HashMap::<String,Rows>::new();
 for (i,n) in names.iter().enumerate(){let rows=parse_rows(&read_file(&mut zip,&format!("xl/worksheets/sheet{}.xml",i+1))?,&ss)?;sheets.insert(n.clone(),rows);}
 let details=sheets.get("规则配置明细").unwrap();let params=index_by_name(sheets.get("告警参数配置").unwrap(),"B");
 let overview=index_by_name(sheets.get("严重告警清单").unwrap(),"E");
 let mut rules=Vec::new();let mut all=0;
 for r in details{
  let rn=get(r,"#row").parse::<u32>().unwrap_or(0);
  if rn<5||get(r,"B").is_empty()||get(r,"H").is_empty(){continue}all+=1;
  let style=get(r,"#style").parse::<usize>().unwrap_or(0);if !reds.contains(&style){continue}
  let name=get(r,"B").trim().to_string();let p=params.get(&name);let o=overview.get(&name);
  let v=|m:Option<&HashMap<String,String>>,key:&str|m.map(|r|get(r,key).to_string()).unwrap_or_default();
  let orig=v(p,"G");let overview_d=v(o,"I");let duration=if orig.is_empty(){overview_d.clone()}else{orig.clone()};
  let mut issues=vec![];
  if p.is_none(){issues.push(issue("error","告警参数配置未找到对应规则"))}
  if o.is_none(){issues.push(issue("error","严重告警清单未找到对应规则"))}
  if orig!=overview_d&&!orig.is_empty()&&!overview_d.is_empty(){issues.push(issue("error",format!("持续时间冲突：参数表 {orig} / 清单 {overview_d}")))}
  let parsed_duration=seconds(&duration);
  let duration_seconds=parsed_duration.unwrap_or(0);
  if parsed_duration.is_none() {issues.push(issue("error",format!("原始持续时间无法解析：{duration}")))}
  if get(r,"H").contains("待确认"){issues.push(issue("error","原始 PromQL 含待确认占位符"))}
  if get(r,"H")!=get(r,"I"){issues.push(issue("warning","原始 PromQL 与建议查询条件不同，按用户要求使用原始值"))}
  if get(r,"M").contains("待确认")||get(r,"N").contains("待确认")||get(r,"O").contains("待确认"){issues.push(issue("warning","接收组待确认：不自动关联通知规则"))}
  if v(p,"O").contains("待确认"){issues.push(issue("warning","扩展标签存在占位符：不会作为静态标签写入"))}
  let rule=Rule{row:rn,name,group:get(r,"C").into(),exporter:v(o,"B"),category:get(r,"D").into(),promql:get(r,"H").into(),suggested_promql:get(r,"I").into(),duration,duration_seconds,suggested_duration:v(p,"H"),summary:get(r,"J").into(),description:get(r,"K").into(),suggestion:get(r,"L").into(),note:get(r,"R").into(),notification:format!("{} / {} / {}",get(r,"M"),get(r,"N"),get(r,"O")),label_requirements:v(p,"M"),recovery:v(p,"J"),repeat:v(p,"K"),issues};
  rules.push(rule)
 }
 if all!=104||rules.len()!=54{return Err(format!("模板校验失败：实际识别全部 {all} 条，红色 {} 条；预期 104/54，拒绝导入",rules.len()))}
 let mut warnings=vec!["仅将规则配置明细A列红色填充对应的54条作为导入候选；所有11个Sheet均已读取".into(),"通知路由、消息模板、采集侧标签与待确认事项只作为校验上下文，不自动写入夜莺".into()];
 if sheets.get("待确认事项").map(|a|a.len()).unwrap_or(0)>4{warnings.push("待确认事项存在，请先人工核对生产前置条件".into())}
 Ok(Preview{preview_id:String::new(),sheets:names,selected:rules.len(),all_rules:all,rules,workbook_warnings:warnings})
}
#[cfg(test)]mod tests{use super::*;
 #[test]fn durations(){assert_eq!(seconds("1m"),Some(60));assert_eq!(seconds("2 分钟"),Some(120));assert_eq!(seconds(""),None);assert_eq!(seconds("0m"),Some(0));}
 #[test]fn red(){assert!(is_red("FFFF0000"));assert!(!is_red("FFD6E4F0"));}
}