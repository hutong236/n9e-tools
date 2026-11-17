// Browser UI state regression tests (no external dependencies).
const {test}=require("node:test");
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const html=fs.readFileSync(path.join(__dirname,"..","static","index.html"),"utf8");
const script=html.split("<script>")[1]?.split("</script>")[0];
assert.ok(script,"Embedded UI script must exist");

function setup(){
 const nodes=new Map();
 function node(id){
  if(!nodes.has(id))nodes.set(id,{
   value:id==="pageSize"?"20":id==="filter"?"all":"",
   textContent:"",innerHTML:"",checked:false,disabled:false,className:"",
   classList:{add(){},remove(){}},addEventListener(){},appendChild(){},showModal(){},close(){}
  });
  return nodes.get(id);
 }
 const document={getElementById:node,createElement(){return {value:"",textContent:""}}};
 // Load UI code into an isolated function with fake DOM. The script does not issue network requests during setup.
 const ui=new Function("document","confirm","alert","fetch",script+
  "\nreturn {init(p){preview=p;selected.clear();paused.clear();removed.clear();for(const r of p.rules)if(isValid(r))selected.add(r.row);render()},filteredRows,filteredSelected,picks,selected,paused,removed,render};")(
   document,()=>true,()=>{},async()=>{throw Error("network not expected")}
  );
 const rules=Array.from({length:15},(_,i)=>({
  row:i+5,name:"rule"+i,group:i%2?"A":"B",exporter:"node",promql:"up == 0",
  duration:"0m",duration_seconds:0,suggested_promql:"",suggested_duration:"",
  issues:i%3===0?[{level:"warning",message:"different query"}]:[]
 }));
 ui.init({rules,sheets:[],selected:15,all_rules:15,workbook_warnings:[]});
 return {ui,node};
}
test("bulk disable applies only to filtered selected rows",()=>{
 const {ui,node}=setup();
 node("filter").value="warning";ui.render();
 const matched=ui.filteredSelected().map(r=>r.row);
 assert.equal(matched.length,5);
 node("bulkDisable").onclick();
 assert.deepEqual([...ui.paused].sort((a,b)=>a-b),matched.sort((a,b)=>a-b));
 assert.equal(ui.picks().length,10,"Hidden nonmatches remain selected");
});
test("restoring filtered disabled candidates does not enable Nightingale rules",()=>{
 const {ui,node}=setup();
 node("filter").value="warning";ui.render();node("bulkDisable").onclick();
 node("filter").value="paused";ui.render();node("bulkEnable").onclick();
 assert.equal(ui.paused.size,0);
 // UI restoration only affects temporary candidate state, not any Nightingale API.
 assert.equal(ui.picks().length,10,"Restoring candidates must not silently select them");
});
test("bulk delete applies only to filtered selected rows, restore is possible",()=>{
 const {ui,node}=setup();
 node("search").value="rule1";ui.render();
 const matched=ui.filteredSelected().map(r=>r.row);
 assert.equal(matched.length,6);
 node("bulkDelete").onclick();
 assert.equal(ui.removed.size,6);
 assert.equal(ui.picks().length,9,"Hidden nonmatches remain selected");
 node("restore").onclick();
 assert.equal(ui.removed.size,0);
});
test("pagination does not reset cross-page selection",()=>{
 const {ui,node}=setup();
 node("pageSize").value="20";ui.render();
 assert.equal(ui.picks().length,15);
 node("search").value="rule1";ui.render();
 assert.equal(ui.picks().length,15);
 node("search").value="";ui.render();
 assert.equal(ui.picks().length,15);
});

test("connection discovery opens picker and fills both IDs only after confirmation",async()=>{
 const nodes=new Map();
 const events=new Map();
 function node(id){
  if(!nodes.has(id))nodes.set(id,{
    value:id==="pageSize"?"20":id==="filter"?"all":"",
    textContent:"",innerHTML:"",disabled:false,checked:false,className:"",
    classList:{add(){},remove(){}},
    addEventListener(name,handler){events.set(id+":"+name,handler)},
    appendChild(){},
    showModal(){this.open=true},close(){this.open=false}
  });
  return nodes.get(id);
 }
 const document={getElementById:node,createElement(){return {value:"",textContent:""}}};
 const mockFetch=async(url,options)=>{
  assert.equal(url,"/api/discover");
  const credentials=JSON.parse(options.body);
  assert.equal(credentials.url,"http://n9e.internal");
  assert.equal(credentials.token,"test-token");
  assert.equal(Object.hasOwn(credentials,"group_id"),false);
  return {ok:true,json:async()=>({groups:[{id:12,name:"生产业务组",kind:""}],datasources:[{id:27,name:"VM-prod",kind:"prometheus"}]})};
 };
 new Function("document","confirm","alert","fetch",script)(document,()=>true,()=>{},mockFetch);
 node("url").value="http://n9e.internal";
 node("token").value="test-token";
 await node("testConnection").onclick();
 assert.equal(node("targetDialog").open,true,"Successful discovery opens the selection dialog");
 assert.equal(node("group").value,"","No auto-selection before user confirmation");
 assert.equal(node("datasource").value,"");
 const groupClick=events.get("targetGroups:click");
 const dsClick=events.get("targetDatasources:click");
 assert.ok(groupClick&&dsClick);
 groupClick({target:{closest(){return {dataset:{id:"12",type:"group"}}}}});
 dsClick({target:{closest(){return {dataset:{id:"27",type:"datasource"}}}}});
 assert.equal(node("confirmTargets").disabled,false);
 node("confirmTargets").onclick();
 assert.equal(node("group").value,"12");
 assert.equal(node("datasource").value,"27");
 assert.equal(node("targetDialog").open,false);
 assert.match(node("connectionStatus").textContent,/生产业务组/);
});

test("live import dashboard tracks actual item events and supports result filtering",()=>{
 const nodes=new Map();
 function node(id){
  if(!nodes.has(id))nodes.set(id,{
   value:id==="pageSize"?"20":id==="filter"?"all":id==="importFilter"?"all":"",
   textContent:"",innerHTML:"",disabled:false,checked:false,className:"",
   style:{},setAttribute(){},scrollIntoView(){},
   classList:{add(){},remove(){}},
   addEventListener(){},appendChild(){},showModal(){},close(){}
  });
  return nodes.get(id);
 }
 const document={getElementById:node,createElement(){return {value:"",textContent:""}}};
 const ui=new Function("document","confirm","alert","fetch",script+
  "\nreturn {resetImportJob,importEvent,consumeImportStream,getJob:()=>importJob};")(
   document,()=>true,()=>{},async()=>{throw Error("network is not expected")}
 );
 ui.resetImportJob([5,6,7]);
 assert.equal(node("importProgressBar").disabled,false);
 assert.equal(node("importCreated").textContent,0);
 ui.importEvent({type:"started",total:3});
 ui.importEvent({type:"current",index:1,row:5,name:"new"});
 assert.match(node("importCurrent").textContent,/new/);
 ui.importEvent({type:"item",item:{row:5,name:"new",status:"created",message:"API accepted"}});
 assert.equal(node("importCreated").textContent,1);
 assert.equal(node("importPending").textContent,2);
 assert.equal(node("importPercent").textContent,"33%");
 ui.importEvent({type:"item",item:{row:6,name:"duplicate",status:"skipped",message:"已存在"}});
 ui.importEvent({type:"item",item:{row:7,name:"bad",status:"failed",message:"HTTP 400"}});
 ui.importEvent({type:"complete",total:3,created:1,skipped:1,failed:1});
 assert.equal(node("importPercent").textContent,"100%");
 assert.equal(node("importFailed").textContent,1);
 assert.equal(node("importPending").textContent,0);
 assert.match(node("importState").textContent,/已完成/);
 assert.equal(ui.getJob().items.length,3);
 node("importFilter").value="failed";
 ui.importEvent({type:"item",item:{row:8,name:"next",status:"invalid",message:"expression"}}); 
 assert.match(node("importRows").innerHTML,/bad/);
 assert.doesNotMatch(node("importRows").innerHTML,/duplicate/);
});
test("NDJSON stream parser handles events split between network chunks",async()=>{
 const nodes=new Map();
 function node(id){
  if(!nodes.has(id))nodes.set(id,{
   value:id==="pageSize"?"20":id==="filter"?"all":id==="importFilter"?"all":"",
   textContent:"",innerHTML:"",disabled:false,checked:false,className:"",
   style:{},setAttribute(){},scrollIntoView(){},
   classList:{add(){},remove(){}},addEventListener(){},appendChild(){},showModal(){},close(){}
  });
  return nodes.get(id);
 }
 const document={getElementById:node,createElement(){return {value:"",textContent:""}}};
 const ui=new Function("document","confirm","alert","fetch",script+
  "\nreturn {resetImportJob,consumeImportStream,getJob:()=>importJob};")(
   document,()=>true,()=>{},async()=>{throw Error("network is not expected")}
 );
 ui.resetImportJob([5]);
 const payload=[
  JSON.stringify({type:"started",total:1}),
  JSON.stringify({type:"item",item:{row:5,name:"one",status:"created",message:"ok"}}),
  JSON.stringify({type:"complete",total:1,created:1,skipped:0,failed:0})
 ].join("\n")+"\n";
 const bytes=new TextEncoder().encode(payload);
 const slices=[bytes.slice(0,13),bytes.slice(13,26),bytes.slice(26)];
 let index=0;
 const response={ok:true,body:{getReader(){return {async read(){return index<slices.length?{value:slices[index++],done:false}:{done:true}}}}}};
 await ui.consumeImportStream(response);
 assert.equal(ui.getJob().phase,"complete");
 assert.equal(ui.getJob().processed,1);
 assert.equal(ui.getJob().created,1);
});

test("rule detail allows J/K/L edits and keeps M collector requirements separate from static tags",()=>{
 const nodes=new Map();
 function node(id){
  if(!nodes.has(id))nodes.set(id,{
   value:id==="pageSize"?"20":id==="filter"||id==="importFilter"?"all":"",
   checked:false,textContent:"",innerHTML:"",disabled:false,className:"",
   style:{},setAttribute(){},scrollIntoView(){},
   classList:{add(){},remove(){}},addEventListener(){},appendChild(){},
   showModal(){this.open=true},close(){this.open=false}
  });
  return nodes.get(id);
 }
 const document={getElementById:node,createElement(){return {value:"",textContent:""}}};
 const ui=new Function("document","confirm","alert","fetch",script+
  "\nreturn {init(p){preview=p;render()},showDetail,saveRuleParams,paramsPayload,currentParams,ruleParams,validStaticTags};")(
   document,()=>true,()=>{},async()=>{throw Error("unexpected fetch")}
  );
 const rule={row:5,name:"Linux-NodeDown",group:"node-exporter",exporter:"node",promql:"up == 0",
   duration:"1m",duration_seconds:60,recovery:"5 分钟",repeat:"60 分钟",max_repeat:"0（不限）",
   recovery_seconds:300,repeat_minutes:60,max_repeat_count:0,
   label_requirements:"host={{主机名}}；env=prod",issues:[]};
 ui.init({rules:[rule],sheets:[],workbook_warnings:[]});
 ui.showDetail(rule);
 assert.match(node("detailBody").innerHTML,/留观时长/);
 assert.match(node("detailBody").innerHTML,/采集侧必备标签/);
 node("paramRecovery").value="600";
 node("paramRepeat").value="45";
 node("paramLimit").value="3";
 node("paramLabelRequirements").value="host={{主机名}}；env=prod";
 node("paramTags").value="team=ops";
 node("paramLabelConfirmed").checked=true;
 ui.saveRuleParams(rule);
 const params=ui.paramsPayload([5])[5];
 assert.equal(params.recover_duration,600);
 assert.equal(params.notify_repeat_step,45);
 assert.equal(params.notify_max_number,3);
 assert.deepEqual(params.append_tags,["team=ops"]);
 assert.equal(params.label_requirements,"host={{主机名}}；env=prod");
 assert.equal(params.labels_confirmed,true);
 assert.throws(()=>ui.validStaticTags("host={{主机名}}"),/附加标签格式/);
});

test("invert selection toggles only eligible filtered rules across pagination",()=>{
 const {ui,node}=setup();
 node("filter").value="warning";ui.render();
 const warningRows=ui.filteredRows().map(r=>r.row);
 assert.equal(warningRows.length,5);
 node("invert").onclick();
 assert.equal(ui.picks().length,10);
 assert.ok(warningRows.every(row=>!ui.selected.has(row)));
 node("invert").onclick();
 assert.equal(ui.picks().length,15);
 node("filter").value="all";
 node("search").value="rule1";ui.render();
 node("invert").onclick();
 assert.equal(ui.picks().length,9);
 node("search").value="";ui.render();
 assert.equal(ui.picks().length,9);
});
test("invert selection ignores paused and invalid rules",()=>{
 const {ui,node}=setup();
 const rules=Array.from({length:15},(_,i)=>({
  row:i+5,name:"rule"+i,group:i%2?"A":"B",exporter:"node",promql:"up",
  duration:"1m",duration_seconds:60,issues:[]
 }));
 rules.push({row:100,name:"invalid",group:"A",exporter:"node",promql:"",
  duration:"",duration_seconds:0,issues:[{level:"error",message:"missing"}]});
 ui.init({rules,sheets:[],selected:16,all_rules:16,workbook_warnings:[]});
 ui.paused.add(5);ui.selected.delete(5);
 ui.render();
 node("invert").onclick();
 assert.equal(ui.paused.has(5),true);
 assert.equal(ui.selected.has(5),false);
 assert.equal(ui.selected.has(100),false);
 assert.equal(ui.selected.size,0);
});

test("placeholder example is not treated as configured append_tags",()=>{
 assert.doesNotMatch(html,/id="paramTags"[^>]*placeholder="team=ops/);
 assert.match(script,/append_tags:\[\],labels_confirmed:false/);
});
test("import event keeps tag readback audit information",()=>{
 const nodes=new Map();
 function node(id){
  if(!nodes.has(id))nodes.set(id,{
   value:id==="pageSize"?"20":id==="filter"||id==="importFilter"?"all":"",
   textContent:"",innerHTML:"",disabled:false,checked:false,className:"",
   style:{},setAttribute(){},scrollIntoView(){},
   classList:{add(){},remove(){}},addEventListener(){},appendChild(){},showModal(){},close(){}
  });
  return nodes.get(id);
 }
 const doc={getElementById:node,createElement(){return {value:"",textContent:""}}};
 const ui=new Function("document","confirm","alert","fetch",script+
  "\nreturn {resetImportJob,importEvent,getJob:()=>importJob};")(doc,()=>true,()=>{},async()=>{throw Error("unwanted network")});
 ui.resetImportJob([5]);
 ui.importEvent({type:"item",item:{row:5,name:"test",status:"created",message:"标签未发现",
  tags_requested:["team=ops"],tags_verification:"mismatch"}});
 assert.deepEqual(ui.getJob().items[0].tags_requested,["team=ops"]);
 assert.equal(ui.getJob().items[0].tags_verification,"mismatch");
 assert.match(node("importRows").innerHTML,/标签待核验/);
 assert.equal(node("importCreated").textContent,1);
});
