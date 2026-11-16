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
