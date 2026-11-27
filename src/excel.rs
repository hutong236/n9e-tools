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
fn index_by_name(rows:&Rows,col:&str)->HashMap<String,HashMap<String,String>>{rows.iter().filter(|r|r.get("#row").and_then(|v|v.parse::<u32>().ok()).unwrap_or(0)>=5).filter(|r|!get(r,col).is_empty()&&!is_summary_row(r)&&!is_template_example(r)).map(|r|(get(r,col).trim().to_string(),r.clone())).collect()}
// Locate the two Excel metadata columns by their header text, not assumed column positions.
// Templates may put them on different sheets or columns; never confuse them with the
// hardcoded Nightingale severity used when creating an alert.
// Summary rows are not alert rules even when column B contains a count.
// Match the dedicated marker in column A only, so a real rule name
// containing "合计" in column B is not silently dropped.
const TEMPLATE_EXAMPLE_MARKER:&str="示例（不导入）";
fn is_template_example(row:&HashMap<String,String>)->bool{
 get(row,"A").trim()==TEMPLATE_EXAMPLE_MARKER
}
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

const TEMPLATE_SHEETS:[&str;11]=["说明","严重告警清单","规则配置明细","告警参数配置","告警参数规范","采集侧标签配置","告警侧派生标签","host_ip 可派生性","通知路由配置","消息模板","待确认事项"];

// The downloadable workbook is generated from the parser's schema, not from
// private production data. A clearly marked example is excluded by the parser.
fn template_rows()->Vec<Vec<Vec<&'static str>>>{
 let mut sheets=vec![
  vec![
   vec!["Nightingale v8 告警规则导入模板"],
   vec!["填写说明：保留全部 11 个工作表及列顺序；第 5 行为示例，请从第 6 行开始录入正式规则。"],
   vec!["步骤","填写要求","示例（仅供参考，不会导入）"],
   vec!["0. 完整案例","三个业务工作表第 5 行为同一条案例，A 列标记为「示例（不导入）」，解析时自动跳过。正式规则请从第 6 行填写，或复制案例并将 A 列改为数字序号，同时替换规则名称和配置。","Example-TargetDown：监控采集目标不可达"],
   vec!["1. 规则配置明细","B 列告警名称、H 列原始 PromQL 必填；J/K/L 列可填写摘要、描述、处理建议。","Example-TargetDown；up == 0"],
   vec!["2. 告警参数配置","B 列名称必须与规则配置明细完全一致；G/J/K/L 列必填。","Example-TargetDown；1m；300s；60 分钟；0（不限）"],
   vec!["3. 严重告警清单","可选：按 E 列规则名称关联 B 列 Exporter、I 列持续时间（G 列原始时长为空时备用）。","Example-TargetDown；node-exporter；1m"],
   vec!["4. 级别与严重程度","规则配置明细 F/G 列用于筛选；不影响新建规则固定 S1 的设置。","严重；S1"],
   vec!["5. 时间格式","持续时间支持 0m；留观时长支持秒/分钟；重复通知必须为整分钟。","0m；5 分钟；60 分钟"],
   vec!["6. 重复次数","填写非负整数，0 表示不限；重复通知间隔填 0 表示不重复。","0（不限）"],
   vec!["7. 标签与通知","静态附加标签在工具详情中配置。采集侧标签及通知接收组仅供参考；新建规则使用新版通知模式，须人工关联通知规则。","team=ops"],
   vec!["8. 安全","新建规则默认 Disabled；同名规则跳过；不要在模板内填写 Token 或凭据。"],
   vec!["9. 辅助工作表","其余工作表仅保留兼容结构，可留空；无需人工构造生产数据。"],
  ],
  vec![vec!["严重告警清单"],vec!["可选参考清单；E 列名称应与规则配置明细 B 列一致。"],vec![],vec!["序号","Exporter","告警分组","监控对象","告警名称（夜莺 Rule）","严重程度","级别","原始告警名","持续时长 for（清单现值）"]],
  vec![vec!["规则配置明细"],vec!["B/H 必填；F/G 仅用于筛选。第 5 行为示例；正式规则从第 6 行填写，不要修改列顺序。"],vec![],vec!["序号","告警名称（夜莺 Rule）","告警分组","监控对象","原始告警名","严重程度","级别","原始 PromQL","建议查询（仅参考）","告警摘要","告警描述","处理建议","通知接收组（仅参考）","通知渠道（仅参考）","通知路由（仅参考）","预留","预留","备注"]],
  vec![vec!["告警参数配置"],vec!["B/G/J/K/L 必填；B 列必须与规则配置明细完全一致。第 5 行为示例；正式规则从第 6 行填写。"],vec![],vec!["序号","告警名称（夜莺 Rule）","告警分组","原始告警名","告警阈值","故障性质","持续时长 for（清单现值）","建议持续时长","持续时长差异","留观时长","重复通知间隔","最大重复次数","采集侧必备标签（仅参考）","预留","告警侧扩展标签（仅参考）"]],
 ];
 // The same sample is present in all three related sheets.
 sheets[1].push(vec![TEMPLATE_EXAMPLE_MARKER,"node-exporter","主机监控","主机","Example-TargetDown","严重","S1","TargetDown","1m"]);
 sheets[2].push(vec![TEMPLATE_EXAMPLE_MARKER,"Example-TargetDown","主机监控","主机","TargetDown","严重","S1",r#"up{job="node-exporter"} == 0"#,"","采集目标不可达","node-exporter 采集目标连续 1 分钟不可达，请检查目标与采集链路。","检查目标状态、网络连通性及 exporter 服务。","按实际情况配置","","","","","示例规则，不导入"]);
 sheets[3].push(vec![TEMPLATE_EXAMPLE_MARKER,"Example-TargetDown","主机监控","TargetDown","up == 0","目标不可达","1m","","","5 分钟","60 分钟","0（不限）","instance、job","",""]);
 for &name in &TEMPLATE_SHEETS[4..]{
  sheets.push(vec![vec![name],vec!["辅助工作表（兼容保留，当前工具不导入本表内容，可留空）"]]);
 }
 sheets
}
fn xml_text(value:&str)->String{
 value.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;")
}
fn xlsx_part(zip:&mut zip::ZipWriter<Cursor<Vec<u8>>>,path:&str,xml:&str)->Result<(),String>{
 use std::io::Write;
 zip.start_file(path,zip::write::SimpleFileOptions::default()).map_err(|e|e.to_string())?;
 zip.write_all(xml.as_bytes()).map_err(|e|e.to_string())
}
fn build_template_workbook(sheets:&[Vec<Vec<&str>>])->Result<Vec<u8>,String>{
 const DECL:&str=r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
 const NS:&str="http://schemas.openxmlformats.org/spreadsheetml/2006/main";
 const REL:&str="http://schemas.openxmlformats.org/officeDocument/2006/relationships";
 const PKG_REL:&str="http://schemas.openxmlformats.org/package/2006/relationships";
 let mut zip=zip::ZipWriter::new(Cursor::new(Vec::new()));
 let mut types=format!(r#"{DECL}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#);
 let mut workbook=format!(r#"{DECL}<workbook xmlns="{NS}" xmlns:r="{REL}"><bookViews><workbookView activeTab="0"/></bookViews><sheets>"#);
 let mut relations=format!(r#"{DECL}<Relationships xmlns="{PKG_REL}">"#);
 for (i,name) in TEMPLATE_SHEETS.iter().enumerate(){
  let n=i+1;
  types.push_str(&format!(r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#));
  workbook.push_str(&format!(r#"<sheet name="{name}" sheetId="{n}" r:id="rId{n}"/>"#));
  relations.push_str(&format!(r#"<Relationship Id="rId{n}" Type="{REL}/worksheet" Target="worksheets/sheet{n}.xml"/>"#));
 }
 types.push_str("</Types>");workbook.push_str("</sheets></workbook>");
 relations.push_str(&format!(r#"<Relationship Id="rId12" Type="{REL}/styles" Target="styles.xml"/></Relationships>"#));
 xlsx_part(&mut zip,"[Content_Types].xml",&types)?;
 xlsx_part(&mut zip,"_rels/.rels",&format!(r#"{DECL}<Relationships xmlns="{PKG_REL}"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#))?;
 xlsx_part(&mut zip,"xl/workbook.xml",&workbook)?;
 xlsx_part(&mut zip,"xl/_rels/workbook.xml.rels",&relations)?;
 let styles=format!(r#"{DECL}<styleSheet xmlns="{NS}"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><color rgb="FFFFFFFF"/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FF203A60"/><bgColor indexed="64"/></patternFill></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="3"><xf numFmtId="49" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="49" fontId="1" fillId="2" borderId="0" xfId="0" applyFont="1" applyFill="1" applyAlignment="1"><alignment vertical="center" wrapText="1"/></xf><xf numFmtId="49" fontId="0" fillId="0" borderId="0" xfId="0" applyAlignment="1"><alignment vertical="center" wrapText="1"/></xf></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#);
 xlsx_part(&mut zip,"xl/styles.xml",&styles)?;
 for (idx,rows) in sheets.iter().enumerate(){
  let n=idx+1;
  let data_sheet=(1..=3).contains(&idx);
  let mut xml=format!(r#"{DECL}<worksheet xmlns="{NS}">"#);
  if data_sheet{
   xml.push_str(r#"<sheetViews><sheetView workbookViewId="0"><pane ySplit="4" topLeftCell="A5" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>"#);
  }
  xml.push_str(r#"<sheetFormatPr defaultRowHeight="18"/><cols>"#);
  let widths:Vec<u32>=match idx{0=>vec![28,100,52],1=>vec![10,24,24,24,38,16,14,28,28],2=>vec![10,38,22,22,28,16,14,66,54,36,54,54,28,24,28,16,16,42],3=>vec![10,38,22,28,40,20,28,24,28,20,22,20,40,16,40],_=>vec![100]};
  for (col,width) in widths.iter().enumerate(){
   let c=col+1;xml.push_str(&format!(r#"<col min="{c}" max="{c}" width="{width}" style="0" customWidth="1"/>"#));
  }
  xml.push_str("</cols><sheetData>");
  for (r,cells) in rows.iter().enumerate(){
   let row=r+1;
   let header=(data_sheet&&row==4)||(idx==0&&row==3);
   let height=if header{r#" ht="32" customHeight="1""#}else if idx==0&&row>=4{r#" ht="36" customHeight="1""#}else{""};
   xml.push_str(&format!(r#"<row r="{row}"{height}>"#));
   for (c,value) in cells.iter().enumerate(){
    if value.is_empty(){continue}
    // All template sheets have fewer than 26 columns.
    let col=(b'A'+c as u8) as char;
    let style=if header{1}else if idx==0&&row>=4{2}else{0};
    xml.push_str(&format!(r#"<c r="{col}{row}" s="{style}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,xml_text(value)));
   }
   xml.push_str("</row>");
  }
  xml.push_str("</sheetData></worksheet>");
  xlsx_part(&mut zip,&format!("xl/worksheets/sheet{n}.xml"),&xml)?;
 }
 zip.finish().map(|c|c.into_inner()).map_err(|e|e.to_string())
}
pub fn download_template()->Result<Vec<u8>,String>{
 build_template_workbook(&template_rows())
}

pub fn parse(data:&[u8])->Result<Preview,String>{
 if data.len()>16*1024*1024{return Err("文件超过16MB限制".into())}
 let mut zip=ZipArchive::new(Cursor::new(data)).map_err(|e|format!("不是有效xlsx：{e}"))?;
 let ss=if zip.file_names().any(|n|n=="xl/sharedStrings.xml"){shared_strings(&read_file(&mut zip,"xl/sharedStrings.xml")?)?}else{vec![]};
 let xml=read_file(&mut zip,"xl/workbook.xml")?;
 let w=roxmltree::Document::parse(&xml).map_err(|e|e.to_string())?;
 let names:Vec<String>=w.descendants().filter(|n|n.has_tag_name("sheet")).filter_map(|n|n.attribute("name").map(str::to_owned)).collect();
 let expected=TEMPLATE_SHEETS;
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
  if rn<5||get(r,"B").trim().is_empty()||is_summary_row(r)||is_template_example(r){continue}all+=1;
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
 #[test]fn downloadable_template_skips_example_and_accepts_filled_rows(){
  let bytes=download_template().unwrap();
  let preview=parse(&bytes).unwrap();
  assert_eq!(preview.sheets,TEMPLATE_SHEETS.map(str::to_owned));
  assert!(preview.rules.is_empty());
  let mut zip=ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
  for part in ["[Content_Types].xml","_rels/.rels","xl/_rels/workbook.xml.rels","xl/styles.xml"]{
   let xml=read_file(&mut zip,part).unwrap();
   roxmltree::Document::parse(&xml).unwrap();
  }
  let mut rows=template_rows();
  assert_eq!(rows[1][4][0],TEMPLATE_EXAMPLE_MARKER);
  assert_eq!(rows[2][4][1],"Example-TargetDown");
  assert_eq!(rows[3][4][1],"Example-TargetDown");
  // Copy the example into a real rule with the same name to verify that
  // marked example rows are excluded from both rule parsing and name indexes.
  rows[1].push(vec!["1","test-exporter","node","host","Example-TargetDown","严重","S1","","1m"]);
  rows[2].push(vec!["1","Example-TargetDown","node","host","","严重","S1","up == 0","","目标不可达","指标为零","检查采集"]);
  rows[3].push(vec!["1","Example-TargetDown","","","","","1m","","","5 分钟","60 分钟","0（不限）"]);
  let filled=build_template_workbook(&rows).unwrap();
  let preview=parse(&filled).unwrap();
  assert_eq!(preview.all_rules,1);
  let rule=&preview.rules[0];
  assert_eq!(rule.name,"Example-TargetDown");
  assert_eq!(rule.row,6);
  assert_eq!(rule.exporter,"test-exporter");
  assert_eq!(rule.promql,"up == 0");
  assert_eq!(rule.duration_seconds,60);
  assert_eq!(rule.recovery_seconds,300);
  assert_eq!(rule.repeat_minutes,60);
  assert_eq!(rule.max_repeat_count,0);
  assert_eq!(rule.excel_level,"S1");
  assert_eq!(rule.excel_severity,"严重");
  assert!(rule.issues.is_empty());
  // Removing the marker on all three sample rows makes the example a usable rule.
  let mut rows=template_rows();
  for idx in 1..=3{rows[idx][4][0]="1";}
  let preview=parse(&build_template_workbook(&rows).unwrap()).unwrap();
  assert_eq!(preview.all_rules,1);
  assert!(preview.rules[0].issues.is_empty());
  assert_eq!(preview.rules[0].promql,r#"up{job="node-exporter"} == 0"#);
 }
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
