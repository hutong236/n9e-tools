//! Parse all valid rules from the supplied 11-sheet XLSX without color filtering.
use serde::{Deserialize,Serialize};
use std::{collections::{HashMap,HashSet},io::{Cursor,Read}};
use zip::ZipArchive;

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Issue { pub level:String, pub message:String }
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Rule {
    pub row:u32,pub name:String,pub group:String,pub exporter:String,pub category:String,
    pub excel_level:String,pub excel_severity:String,
    pub promql:String,pub suggested_promql:String,pub duration:String,pub duration_seconds:u64,
    pub suggested_duration:String,pub summary:String,pub description:String,pub suggestion:String,
    pub note:String,pub notification:String,pub label_requirements:String,
    pub recovery:String,pub repeat:String,pub max_repeat:String,
    pub recovery_seconds:u64,pub repeat_minutes:u64,pub max_repeat_count:u64,pub issues:Vec<Issue>,
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
   if !value.is_empty(){row.insert(col.clone(),value);}
   if col=="A"{row.insert("#style".into(),c.attribute("s").unwrap_or("0").into());}
  }
  out.push(row)
 }Ok(out)
}
fn get<'a>(r:&'a HashMap<String,String>,column:&str)->&'a str{r.get(column).map(|s|s.as_str()).unwrap_or("")}
fn index_by_name(rows:&Rows,col:&str)->HashMap<String,HashMap<String,String>>{rows.iter().filter(|r|r.get("#row").and_then(|v|v.parse::<u32>().ok()).unwrap_or(0)>=5).filter(|r|!get(r,col).is_empty()).map(|r|(get(r,col).trim().to_string(),r.clone())).collect()}
// Locate the two Excel metadata columns by their header text, not assumed column positions.
// Templates may put them on different sheets or columns; never confuse them with the
// hardcoded Nightingale severity used when creating an alert.
// Summary rows are not alert rules even when column B contains a count.
// Match the dedicated marker in column A only, so a real rule name
// containing "合计" in column B is not silently dropped.
fn is_summary_row(row:&HashMap<String,String>)->bool{
 get(row,"A").trim()=="合计"
}
fn metadata_header(rows:&Rows,label:&str)->Option<String>{
 rows.iter().filter(|r|get(r,"#row").parse::<u32>().unwrap_or(u32::MAX)<5)
  .find_map(|r|r.iter().find_map(|(col,value)|{
   (col!="#row" && col!="#style" && value.trim()==label).then(||col.clone())
  }))
}
fn metadata_value(row:&HashMap<String,String>,column:Option<&str>)->String{
 column.map(|c|get(row,c).trim().to_string()).unwrap_or_default()
}
pub fn seconds(v:&str)->Option<u64>{let s=v.trim();if s.is_empty(){return None};
 if let Some(a)=s.strip_suffix("ms"){return a.trim().parse::<u64>().ok().map(|x|x/1000)}
 for (suffix,factor) in [("分钟",60),("小时",3600),("秒",1),("m",60),("h",3600),("s",1),("d",86400)]{
  if let Some(a)=s.strip_suffix(suffix){return a.trim().parse::<u64>().ok().map(|x|x*factor)}
 }s.parse::<u64>().ok()
}
pub fn repeat_count(value:&str)->Option<u64>{
 let raw=value.trim();let digits:String=raw.chars().take_while(|c|c.is_ascii_digit()).collect();
 if digits.is_empty(){return None}
 let suffix=raw[digits.len()..].trim();
 if !suffix.is_empty()&&suffix!="（不限）"&&suffix!="(不限)"{return None}
 digits.parse().ok()
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
 let mut sheets=HashMap::<String,Rows>::new();
 for (i,n) in names.iter().enumerate(){let rows=parse_rows(&read_file(&mut zip,&format!("xl/worksheets/sheet{}.xml",i+1))?,&ss)?;sheets.insert(n.clone(),rows);}
 let details=sheets.get("规则配置明细").unwrap();let params=index_by_name(sheets.get("告警参数配置").unwrap(),"B");
 let detail_level_col=metadata_header(details,"级别");
 let detail_severity_col=metadata_header(details,"严重程度");
 let list_rows=sheets.get("严重告警清单").unwrap();
 let list_level_col=metadata_header(list_rows,"级别");
 let list_severity_col=metadata_header(list_rows,"严重程度");
 let overview=index_by_name(sheets.get("严重告警清单").unwrap(),"E");
 let mut rules=Vec::new();let mut all=0;
 for r in details{
  let rn=get(r,"#row").parse::<u32>().unwrap_or(0);
  if rn<5||get(r,"B").trim().is_empty()||is_summary_row(r){continue}all+=1;
  let name=get(r,"B").trim().to_string();let p=params.get(&name);let o=overview.get(&name);
  let v=|m:Option<&HashMap<String,String>>,key:&str|m.map(|r|get(r,key).to_string()).unwrap_or_default();
  let orig=v(p,"G");let overview_d=v(o,"I");let duration=if orig.is_empty(){overview_d.clone()}else{orig.clone()};
  let mut issues=vec![];
  if p.is_none(){issues.push(issue("error","告警参数配置未找到对应规则：影响持续时间和J/K/L参数写入"))}
  let parsed_duration=seconds(&duration);
  let duration_seconds=parsed_duration.unwrap_or(0);
  if parsed_duration.is_none() {issues.push(issue("error",format!("原始持续时间无法解析：{duration}")))}
  if get(r,"H").trim().is_empty(){issues.push(issue("error","原始 PromQL 为空，无法创建告警规则"))}
  if get(r,"H").contains("待确认"){issues.push(issue("error","即将写入的原始 PromQL 含待确认占位符"))}
  let recovery=v(p,"J");let repeat=v(p,"K");let max_repeat=v(p,"L");
  let recovery_parsed=seconds(&recovery);
  let repeat_parsed=seconds(&repeat);
  let repeat_minutes=repeat_parsed.filter(|x|x%60==0).map(|x|x/60);
  let max_repeat_count=repeat_count(&max_repeat);
  if recovery_parsed.is_none(){issues.push(issue("error",format!("告警参数配置 J列：留观时长无法解析：{recovery}")))}
  if repeat_minutes.is_none(){issues.push(issue("error",format!("告警参数配置 K列：重复通知间隔须为整分钟：{repeat}")))}
  if max_repeat_count.is_none(){issues.push(issue("error",format!("告警参数配置 L列：最大重复次数无法解析：{max_repeat}")))}
  if recovery_parsed.is_some_and(|x|x>86400){issues.push(issue("error","留观时长超过24小时，超出实际导入允许范围"))}
  if repeat_minutes.is_some_and(|x|x>10080){issues.push(issue("error","重复通知间隔超过7天，超出实际导入允许范围"))}
  if max_repeat_count.is_some_and(|x|x>10000){issues.push(issue("error","最大重复次数超过10000，超出实际导入允许范围"))}
  let excel_level=metadata_value(r,detail_level_col.as_deref());
  let excel_level=if excel_level.is_empty(){o.map(|r|metadata_value(r,list_level_col.as_deref())).unwrap_or_default()}else{excel_level};
  let excel_severity=metadata_value(r,detail_severity_col.as_deref());
  let excel_severity=if excel_severity.is_empty(){o.map(|r|metadata_value(r,list_severity_col.as_deref())).unwrap_or_default()}else{excel_severity};
  let rule=Rule{row:rn,name,group:get(r,"C").into(),exporter:v(o,"B"),category:get(r,"D").into(),excel_level,excel_severity,promql:get(r,"H").into(),suggested_promql:get(r,"I").into(),duration,duration_seconds,suggested_duration:v(p,"H"),summary:get(r,"J").into(),description:get(r,"K").into(),suggestion:get(r,"L").into(),note:get(r,"R").into(),notification:format!("{} / {} / {}",get(r,"M"),get(r,"N"),get(r,"O")),label_requirements:v(p,"M"),recovery,repeat,max_repeat,recovery_seconds:recovery_parsed.unwrap_or(0),repeat_minutes:repeat_minutes.unwrap_or(0),max_repeat_count:max_repeat_count.unwrap_or(0),issues};
  rules.push(rule)
 }
 let mut warnings=vec!["全部有效规则均列入候选，不按颜色筛选；用户可在预览中自行过滤、禁用、删除".into(),"通知接收组、采集侧标签要求及Excel建议查询只作参考，不写入夜莺，也不计入规则校验状态".into()];

 Ok(Preview{preview_id:String::new(),sheets:names,selected:rules.len(),all_rules:all,rules,workbook_warnings:warnings})
}
#[cfg(test)]mod tests{use super::*;
 #[test]fn durations(){assert_eq!(seconds("1m"),Some(60));assert_eq!(seconds("2 分钟"),Some(120));assert_eq!(seconds(""),None);assert_eq!(seconds("0m"),Some(0));assert_eq!(repeat_count("0（不限）"),Some(0));assert_eq!(repeat_count("3"),Some(3));assert_eq!(repeat_count("无限"),None);}
 #[test]fn red(){assert!(is_red("FFFF0000"));assert!(!is_red("FFD6E4F0"));}
 #[test]fn summary_row_marker_does_not_exclude_genuine_rule_names(){
  let summary=HashMap::from([("A".to_string()," 合计 ".to_string()),("B".to_string(),"104 条严重告警规则".to_string())]);
  assert!(is_summary_row(&summary));
  let normal=HashMap::from([("A".to_string(),"105".to_string()),("B".to_string(),"ipmi_合计功率异常".to_string())]);
  assert!(!is_summary_row(&normal));
 }
 #[test]fn finds_excel_level_and_severity_headers(){
  let rows=vec![
   HashMap::from([("#row".into(),"3".into()),("K".into(),"级别".into()),("L".into(),"严重程度".into())]),
   HashMap::from([("#row".into(),"5".into()),("K".into(),"P1".into()),("L".into(),"严重".into())])
  ];
  assert_eq!(metadata_header(&rows,"级别").as_deref(),Some("K"));
  assert_eq!(metadata_header(&rows,"严重程度").as_deref(),Some("L"));
  assert_eq!(metadata_value(&rows[1],metadata_header(&rows,"级别").as_deref()),"P1");
  assert_eq!(metadata_header(&rows,"不存在"),None);
 }
}

#[cfg(test)]
mod integration_tests {
 use super::*;
 use std::io::{Cursor,Write};
 use zip::write::SimpleFileOptions;

 #[test]
 fn parses_full_red_selection_and_preserves_zero_duration() {
  let sheets=["说明","严重告警清单","规则配置明细","告警参数配置","告警参数规范","采集侧标签配置","告警侧派生标签","host_ip 可派生性","通知路由配置","消息模板","待确认事项"];
  let xml_header=r#"<?xml version="1.0" encoding="UTF-8"?>"#;
  let mut out=Cursor::new(Vec::<u8>::new());
  {
   let mut z=zip::ZipWriter::new(&mut out);
   let opts=SimpleFileOptions::default();
   z.start_file("xl/workbook.xml",opts).unwrap();
   write!(z,"{}<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheets>",xml_header).unwrap();
   for (i,name) in sheets.iter().enumerate(){write!(z,"<sheet name=\"{}\" sheetId=\"{}\"/>",name,i+1).unwrap();}
   write!(z,"</sheets></workbook>").unwrap();
   z.start_file("xl/styles.xml",opts).unwrap();
   write!(z,"{}<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><fills count=\"2\"><fill/><fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFFF0000\"/></patternFill></fill></fills><cellXfs count=\"2\"><xf fillId=\"0\"/><xf fillId=\"1\"/></cellXfs></styleSheet>",xml_header).unwrap();
   for idx in 1..=11 {
    z.start_file(format!("xl/worksheets/sheet{idx}.xml"),opts).unwrap();
    write!(z,"{}<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",xml_header).unwrap();
    if idx==2||idx==3||idx==4 {
     if idx==2||idx==3 {
      write!(z,"<row r=\"4\"><c r=\"S4\" t=\"inlineStr\"><is><t>级别</t></is></c><c r=\"T4\" t=\"inlineStr\"><is><t>严重程度</t></is></c></row>").unwrap();
     }
     for i in 0..104 {
      let row=i+5;let name=format!("synthetic-{i}");
      write!(z,"<row r=\"{row}\">").unwrap();
      if idx==2 {
       write!(z,"<c r=\"E{row}\" t=\"inlineStr\"><is><t>{name}</t></is></c><c r=\"I{row}\" t=\"inlineStr\"><is><t>{}</t></is></c><c r=\"S{row}\" t=\"inlineStr\"><is><t>fallback</t></is></c><c r=\"T{row}\" t=\"inlineStr\"><is><t>清单严重度</t></is></c>",if i<11{"0m"}else{"1m"}).unwrap();
      }else if idx==3 {
       let style=if i<54 {1}else{0};
       write!(z,"<c r=\"A{row}\" s=\"{style}\"><v>{}</v></c><c r=\"B{row}\" t=\"inlineStr\"><is><t>{name}</t></is></c><c r=\"H{row}\" t=\"inlineStr\"><is><t>vector(1) > 0</t></is></c><c r=\"I{row}\" t=\"inlineStr\"><is><t>vector(2) > 0</t></is></c><c r=\"M{row}\" t=\"inlineStr\"><is><t>待确认</t></is></c><c r=\"S{row}\" t=\"inlineStr\"><is><t>{}</t></is></c><c r=\"T{row}\" t=\"inlineStr\"><is><t>{}</t></is></c>",i+1,if i%2==0{"P1"}else{"P2"},if i%2==0{"严重"}else{"一般"}).unwrap();
      }else{
       write!(z,"<c r=\"B{row}\" t=\"inlineStr\"><is><t>{name}</t></is></c><c r=\"G{row}\" t=\"inlineStr\"><is><t>{}</t></is></c><c r=\"J{row}\" t=\"inlineStr\"><is><t>5 分钟</t></is></c><c r=\"K{row}\" t=\"inlineStr\"><is><t>60 分钟</t></is></c><c r=\"L{row}\" t=\"inlineStr\"><is><t>0（不限）</t></is></c><c r=\"M{row}\" t=\"inlineStr\"><is><t>host={{{{主机名}}}}</t></is></c><c r=\"O{row}\" t=\"inlineStr\"><is><t>待确认</t></is></c>",if i<11{"0m"}else{"1m"}).unwrap();
      }
      write!(z,"</row>").unwrap();
     }
     if idx==3 {
      // The actual workbook has a merged summary row at Excel row 109.
      // Column B is populated but it must not count as an alert rule.
      write!(z,"<row r=\"109\"><c r=\"A109\" t=\"inlineStr\"><is><t>合计</t></is></c><c r=\"B109\" t=\"inlineStr\"><is><t>104 条严重告警规则</t></is></c><c r=\"C109\" t=\"inlineStr\"><is><t>20 个告警分组</t></is></c></row>").unwrap();
     }
    }
    write!(z,"</sheetData></worksheet>").unwrap();
   }
   z.finish().unwrap();
  }
  let preview=parse(&out.into_inner()).unwrap();
  assert_eq!(preview.sheets.len(),11);
  assert_eq!(preview.all_rules,104);
  assert_eq!(preview.selected,104);
  assert_eq!(preview.rules.len(),104);
  assert_eq!(preview.rules.iter().filter(|r|r.duration_seconds==0).count(),11);
  assert_eq!(preview.rules[0].promql,"vector(1) > 0");
  assert_eq!(preview.rules[103].row,108);
  assert!(!preview.rules.iter().any(|r|r.row==109||r.name.contains("104 条严重告警规则")));
  assert_eq!(preview.rules[0].recovery_seconds,300);
  assert_eq!(preview.rules[0].repeat_minutes,60);
  assert_eq!(preview.rules[0].max_repeat_count,0);
  assert_eq!(preview.rules[0].label_requirements,"host={{主机名}}");
  assert_eq!(preview.rules[0].excel_level,"P1");
  assert_eq!(preview.rules[0].excel_severity,"严重");
  assert_eq!(preview.rules[1].excel_level,"P2");
  assert_eq!(preview.rules[1].excel_severity,"一般");
  assert!(preview.rules[0].issues.iter().all(|i|i.level!="error"));
  assert!(preview.rules.iter().all(|r|r.issues.is_empty()),"辅助标签占位符、通知待确认与建议PromQL差异不应生成规则校验警告");
 }
}
