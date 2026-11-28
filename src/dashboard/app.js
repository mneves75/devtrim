"use strict";(()=>{
const snapshot=JSON.parse(document.getElementById("snapshot").textContent);
const byId=id=>document.getElementById(id);
const text=(id,value)=>{byId(id).textContent=value};
const make=(tag,value,className)=>{const element=document.createElement(tag);if(value!==undefined)element.textContent=value;if(className)element.className=className;return element};
const fmt0=new Intl.NumberFormat(undefined,{maximumFractionDigits:0});
const fmt1=new Intl.NumberFormat(undefined,{maximumFractionDigits:1});
const units=["B","KiB","MiB","GiB","TiB","PiB","EiB"];
const size=value=>{if(value===null||value===undefined)return"Unavailable";let unit=0;while(value>=1024&&unit<units.length-1){value/=1024;unit++}return`${(unit?fmt1:fmt0).format(value)} ${units[unit]}`};
const root=snapshot.inventory;
const parents=new Map();
const index=(node,parent)=>{parents.set(node,parent);for(const child of node.children)index(child,node)};
index(root,null);
let current=root;
let limit=100;
let searchFrame=0;
const findingByPath=new Map();
let order=0;
for(const category of snapshot.recommendations.categories){
for(const finding of category.findings){
if(!findingByPath.has(finding.path))findingByPath.set(finding.path,{category:category.category,kind:finding.kind,order});
order++;
}
}
const agentRe=/\/(\.codex|\.claude|\.pi)(\/|$)/;
const colorCache=new Map();
const color=node=>{
const cached=colorCache.get(node);
if(cached!==undefined)return cached;
let row=null;
for(let ancestor=node;ancestor;ancestor=parents.get(ancestor)){
const hit=findingByPath.get(ancestor.path);
if(hit&&(!row||hit.order<row.order))row=hit;
}
let cls="other";
if(row?.category==="agents"||agentRe.test(node.path))cls="agents";
else if(row&&(row.category==="artifacts"||row.category==="node-modules"||row.category==="xcode"))cls="build";
else if(row?.kind==="review")cls="review";
else if(row)cls="preview";
colorCache.set(node,cls);
return cls;
};
const searchKeys=new Map();
const keyOf=node=>{
let pair=searchKeys.get(node);
if(pair===undefined){pair=[node.name.toLocaleLowerCase(),node.path.toLocaleLowerCase()];searchKeys.set(node,pair)}
return pair;
};
const mapEl=byId("map");
const searchEl=byId("search");
const sortEl=byId("sort");
const foldersEl=byId("folders");
const crumbsEl=byId("breadcrumbs");
const backEl=byId("back");
const moreEl=byId("more");
const capturedAt=new Date(snapshot.generated_unix_seconds*1000);
text("root-path",snapshot.root);
text("report-path",snapshot.output_file);
text("captured",capturedAt.toLocaleString());
byId("captured").dateTime=capturedAt.toISOString();
text("mapped",`${snapshot.coverage.partial?"≥ ":""}${size(root.bytes)}`);
text("coverage-label",snapshot.coverage.partial?"Partial lower bound · some entries unavailable":"Files and symlinks · bytes on disk");
text("available",size(snapshot.volume.available_bytes));
text("capacity",snapshot.volume.capacity_bytes===null?"Volume metrics unavailable; see diagnostics":`of ${size(snapshot.volume.capacity_bytes)} volume capacity`);
text("candidates",snapshot.recommendations.categories.length.toLocaleString());
text("bounds",`Overview: up to ${snapshot.coverage.max_depth} levels and ${snapshot.coverage.max_nodes.toLocaleString()} entries. ${snapshot.coverage.summarized_entries.toLocaleString()} entries have summarized detail; their measured bytes remain included. Symlinks are not followed; foreign filesystems are not traversed.`);
if(snapshot.errors.length){
const banner=byId("banner");
banner.hidden=false;
const notices=[snapshot.coverage.partial?"Map is a partial lower bound.":"Map measurement is complete."];
if(snapshot.volume.error)notices.push("Volume data unavailable.");
if(snapshot.recommendations.errors.length){
const failures=new Set(snapshot.recommendations.errors.map(error=>error.split(":",1)[0]));
notices.push(`${failures.size} cleanup ${failures.size===1?"category unavailable":"categories unavailable"}.`);
}
const details=make("button","Show diagnostics");
details.type="button";
details.addEventListener("click",()=>{const disclosure=byId("diagnostics-disclosure");disclosure.open=true;disclosure.querySelector("summary").focus()});
banner.append(make("span",notices.join(" ")),details);
}
const diagnostics=[...snapshot.coverage.errors,...(snapshot.volume.error?[snapshot.volume.error]:[]),...snapshot.recommendations.errors,snapshot.recommendations.roots_note,...snapshot.recommendations.diagnostics];
if(snapshot.errors.length)diagnostics.unshift("Resolve the access or tool errors below, then rerun devtrim dashboard with the same path. Category preview commands also retry their own probes.");
if(snapshot.coverage.issue_count>snapshot.coverage.errors.length)diagnostics.push(`${snapshot.coverage.issue_count-snapshot.coverage.errors.length} additional inventory issues; individual diagnostics are limited to 256.`);
const diagFrag=document.createDocumentFragment();
for(const message of diagnostics)diagFrag.append(make("p",message));
byId("diagnostics").append(diagFrag);
const info=node=>{text("detail-path",node.path);text("detail-info",`${node.kind} · ${node.partial?"at least ":""}${size(node.bytes)} on disk${node.partial?" · partial measurement":""}${node.summarized_entries?` · ${node.summarized_entries.toLocaleString()} entries summarized here (${size(node.summarized_bytes)})`:""}. Inventory only; no deletion action is attached to this item.`)};
const navigate=node=>{info(node);if(node.kind!=="directory")return;current=node;limit=100;searchEl.value="";cancelAnimationFrame(searchFrame);render();crumbsEl.lastElementChild?.querySelector("button").focus()};
const svgNode=(tag,attributes)=>{const element=document.createElementNS("http://www.w3.org/2000/svg",tag);for(const[key,value]of Object.entries(attributes))element.setAttribute(key,String(value));return element};
const partition=(items,x,y,width,height,draw)=>{
if(!items.length)return;
if(items.length===1){draw(items[0],x,y,width,height);return}
const total=items.reduce((sum,item)=>sum+item.bytes,0);
let split=1,first=items[0].bytes;
while(split<items.length-1&&first+items[split].bytes/2<total/2)first+=items[split++].bytes;
const fraction=first/total;
if(width>=height){partition(items.slice(0,split),x,y,width*fraction,height,draw);partition(items.slice(split),x+width*fraction,y,width*(1-fraction),height,draw)}
else{partition(items.slice(0,split),x,y,width,height*fraction,draw);partition(items.slice(split),x,y+height*fraction,width,height*(1-fraction),draw)}
};
const visibleItems=(node,budget)=>{
const children=node.children.filter(child=>child.bytes>0).sort((left,right)=>right.bytes-left.bytes||left.path.localeCompare(right.path));
if(children.length<=budget&&!node.summarized_bytes)return children;
const shown=children.slice(0,Math.max(0,budget-1));
const other=node.summarized_bytes+children.slice(shown.length).reduce((sum,child)=>sum+child.bytes,0);
if(other)shown.push({name:"Other measured entries",path:node.path,bytes:other,summarized:true,owner:node});
return shown;
};
const shorten=(name,width)=>{const characters=Math.max(1,Math.floor(width/7.5));return name.length>characters?`${name.slice(0,Math.max(0,characters-1))}…`:name};
const tileButton=(node,x,y,width,height,heading=false)=>{
const measured=`${node.partial?"≥ ":""}${size(node.bytes)}`;
const tile=svgNode("g",{class:`tile ${heading?"parent-heading other":node.summarized?"other":color(node)}`,role:"button",tabindex:0,"aria-label":`${node.name}, ${node.partial?"at least ":""}${size(node.bytes)}${node.summarized?", summarized detail":`, ${node.kind}`}`});
tile.append(svgNode("rect",{x,y,width,height,rx:3}));
const title=svgNode("title",{});
title.textContent=`${node.path}\n${measured}`;
tile.append(title);
if(heading){
const name=svgNode("text",{x:x+8,y:y+19});
name.textContent=shorten(node.name,width-measured.length*7-30);
const value=svgNode("text",{x:x+width-8,y:y+19,"text-anchor":"end",class:"tile-size"});
value.textContent=measured;
tile.append(name,value);
}else if(width>=75&&height>=42){
const name=svgNode("text",{x:x+8,y:y+22});
name.textContent=shorten(node.name,width-18);
tile.append(name);
if(height>=62&&width>=95){const label=svgNode("text",{x:x+8,y:y+42,class:"tile-size"});label.textContent=measured;tile.append(label)}
}
const activate=()=>{if(node.summarized){info(node.owner);text("detail-info",`${size(node.bytes)} of measured entries grouped in this map. Retained children remain available in the folder list; summarized descendants need a new dashboard rooted closer to them.`)}else navigate(node)};
tile.addEventListener("click",event=>{event.stopPropagation();activate()});
tile.addEventListener("keydown",event=>{if(event.key==="Enter"||event.key===" "){event.preventDefault();event.stopPropagation();activate()}});
return tile;
};
const drawMap=()=>{
const shown=visibleItems(current,60);
if(!shown.length){mapEl.replaceChildren(make("p",current.partial?"No readable bytes to map. See coverage diagnostics.":"No measured bytes in this folder. Empty and zero-byte entries remain in the list.","empty"));return}
const mapWidth=Math.max(1,mapEl.clientWidth);
const mapHeight=Math.round(Math.max(256,Math.min(440,mapWidth*.6)));
const svg=svgNode("svg",{width:mapWidth,height:mapHeight,viewBox:`0 0 ${mapWidth} ${mapHeight}`,role:"group","aria-label":"Size treemap with one nested folder level"});
const svgFrag=document.createDocumentFragment();
let nestedBudget=180;
partition(shown,0,0,mapWidth,mapHeight,(node,x,y,width,height)=>{
if(node.kind!=="directory"||width<170||height<115||!nestedBudget){svgFrag.append(tileButton(node,x,y,width,height));return}
const children=visibleItems(node,Math.min(16,nestedBudget));
if(!children.length){svgFrag.append(tileButton(node,x,y,width,height));return}
nestedBudget-=children.length;
const folder=svgNode("g",{class:"folder-group",role:"group","aria-label":`${node.name} folder, ${node.partial?"at least ":""}${size(node.bytes)}`});
const folderFrag=document.createDocumentFragment();
folderFrag.append(svgNode("rect",{x,y,width,height,rx:3}),tileButton(node,x+2,y+2,width-4,28,true));
partition(children,x+3,y+32,width-6,height-35,(child,childX,childY,childWidth,childHeight)=>folderFrag.append(tileButton(child,childX,childY,childWidth,childHeight)));
folder.append(folderFrag);
svgFrag.append(folder);
});
svg.append(svgFrag);
mapEl.replaceChildren(svg);
};
const renderList=()=>{
const query=searchEl.value.trim().toLocaleLowerCase();
let entries;
if(query){
entries=[];
const stack=[current];
while(stack.length){
const node=stack.pop();
for(const child of node.children){
const[lowerName,lowerPath]=keyOf(child);
if(lowerName.includes(query)||lowerPath.includes(query))entries.push(child);
if(child.children.length)stack.push(child);
}
}
}else{entries=current.children.slice()}
entries.sort(sortEl.value==="name"?(left,right)=>left.name.localeCompare(right.name)||left.path.localeCompare(right.path):(left,right)=>right.bytes-left.bytes||left.path.localeCompare(right.path));
text("list-count",`${entries.length.toLocaleString()} ${query?"matching retained entries in this subtree":"retained entries in this folder"}${current.summarized_entries?" · additional measured detail summarized":""}`);
const listFrag=document.createDocumentFragment();
for(const node of entries.slice(0,limit)){
const item=make("li");
const button=make("button",undefined,"folder-row");
button.type="button";
const name=make("span",node.name,"folder-name");
name.append(make("span",`${node.kind}${node.partial?" · partial":""}`,`folder-type ${color(node)}`));
button.append(name,make("span",`${node.partial?"≥ ":""}${size(node.bytes)}`,"folder-size"),make("span",node.path,"folder-path"));
button.addEventListener("click",()=>navigate(node));
item.append(button);
listFrag.append(item);
}
if(!entries.length)listFrag.append(make("li",query?"No matching retained entries. Search covers the overview, not summarized descendants.":current.summarized_entries?"Deeper detail is summarized. Generate a new dashboard rooted here for a closer view.":"This folder has no retained children.","empty"));
foldersEl.replaceChildren(listFrag);
moreEl.hidden=entries.length<=limit;
};
const render=()=>{
const crumbs=[];
for(let ancestor=current;ancestor;ancestor=parents.get(ancestor))crumbs.unshift(ancestor);
const crumbFrag=document.createDocumentFragment();
for(const node of crumbs){
const item=make("li");
const button=make("button",node.name);
button.type="button";
button.title=node.path;
if(node===current)button.setAttribute("aria-current","location");
button.addEventListener("click",()=>navigate(node));
item.append(button);
crumbFrag.append(item);
}
crumbsEl.replaceChildren(crumbFrag);
backEl.disabled=!parents.get(current);
text("current-name",current.name);
text("current-size",`${current.partial?"≥ ":""}${size(current.bytes)}`);
drawMap();
renderList();
info(current);
};
backEl.addEventListener("click",()=>{const parent=parents.get(current);if(parent)navigate(parent)});
searchEl.addEventListener("input",()=>{limit=100;cancelAnimationFrame(searchFrame);searchFrame=requestAnimationFrame(renderList)});
sortEl.addEventListener("change",renderList);
moreEl.addEventListener("click",()=>{limit+=100;renderList()});
const recommendations=byId("recommendations");
const recFrag=document.createDocumentFragment();
for(const category of snapshot.recommendations.categories){
const section=make("section",undefined,"category");
section.append(make("h3",category.category.replaceAll("-"," ")));
const list=make("ul");
const itemsFrag=document.createDocumentFragment();
for(const row of category.findings){
const item=make("li");
const details=make("details");
details.append(make("summary","Why this appears"),make("p",row.note));
item.append(make("strong",row.label),make("p",`${row.kind==="review"?"Review":"Category preview"} · ${size(row.bytes)}`),make("p",row.path,"full-path"),details);
itemsFrag.append(item);
}
list.append(itemsFrag);
section.append(list);
if(category.preview_command===null){section.append(make("p",category.preview_unavailable));recFrag.append(section);continue}
if(category.preview_command.includes("$'"))section.append(make("p","Control-character quoting requires zsh, Bash, or a POSIX.1-2024 shell."));
const field=make("input");
field.type="text";
field.readOnly=true;
field.value=category.preview_command;
field.setAttribute("aria-label",`${category.category} preview command`);
const button=make("button","Copy preview command");
button.type="button";
button.addEventListener("click",async()=>{
try{await navigator.clipboard.writeText(category.preview_command);text("copy-status",`Copied ${category.preview_command}. Run it in your terminal for a fresh preview.`)}
catch{field.focus();field.select();text("copy-status","Clipboard unavailable. The preview command is selected; copy it from the field.")}
});
section.append(field,button);
recFrag.append(section);
}
if(!snapshot.recommendations.categories.length)recFrag.append(make("p",snapshot.recommendations.errors.length?"Category probes are incomplete. No matching rows are available; this does not establish that no candidates exist.":"No category findings inside this map. Everything shown in the explorer remains inventory only."));
recommendations.append(recFrag);
render();
let lastWidth=mapEl.clientWidth;
new ResizeObserver(()=>{const width=mapEl.clientWidth;if(width!==lastWidth){lastWidth=width;drawMap()}}).observe(mapEl);
})();
