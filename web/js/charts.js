import {$,setText,formatBytes,formatTime} from "./common.js";

const plots=new Map();

function chartTheme(){
  const s=getComputedStyle(document.documentElement);
  return{muted:s.getPropertyValue("--muted").trim(),border:s.getPropertyValue("--border").trim(),accent:s.getPropertyValue("--accent").trim(),red:s.getPropertyValue("--red").trim(),blue:s.getPropertyValue("--blue").trim(),amber:s.getPropertyValue("--amber").trim(),violet:s.getPropertyValue("--violet").trim()};
}

function chartOptions(host,series,height=270,yFormatter=null){
  const c=chartTheme(),xAxis={stroke:c.muted,grid:{stroke:c.border,width:1},ticks:{stroke:c.border,width:1},font:"11px system-ui"},yAxis={stroke:c.muted,grid:{stroke:c.border,width:1},ticks:{stroke:c.border,width:1},font:"11px system-ui"};
  if(yFormatter)yAxis.values=(_,values)=>values.map(yFormatter);
  return{width:Math.max(280,Math.floor(host.clientWidth)),height,scales:{x:{time:true},y:{auto:true}},axes:[xAxis,yAxis],series,legend:{show:true},cursor:{drag:{x:true,y:false,setScale:true}}};
}

function upsertPlot(key,hostId,data,series,height=270,yFormatter=null){
  const host=$(hostId);if(!host||host.hidden||host.clientWidth===0)return;
  const existing=plots.get(key);if(existing){existing.setData(data);return;}
  plots.set(key,new window.uPlot(chartOptions(host,series,height,yFormatter),data,host));
}

function percentileText(value){if(!value||value.samples===0)return"—";return `${value.p50??0} / ${value.p95??0} / ${value.p99??0} ms`;}

export function renderMetrics(data){
  const totals=data.totals||{},points=data.points||[];
  setText("activity-tool-latency",percentileText(data.latency?.tool_ms));
  setText("activity-queue-wait",percentileText(data.latency?.queue_wait_ms));
  setText("activity-project-busy",data.counters?.project_busy??0);
  setText("activity-job-stops",`${data.counters?.jobs?.timed_out??0} / ${data.counters?.jobs?.cancelled??0} / ${data.counters?.jobs?.interrupted??0}`);
  setText("activity-git-commit",`${data.counters?.git?.commit?.succeeded??0} / ${data.counters?.git?.commit?.failed??0}`);
  setText("activity-git-push",`${data.counters?.git?.push?.succeeded??0} / ${data.counters?.git?.push?.failed??0}`);
  for(const key of ["requests","success","failed","http-requests","rx","tx"]){
    const value=key==="requests"?totals.requests:key==="success"?totals.successes:key==="failed"?totals.failures:key==="http-requests"?totals.http_requests:key==="rx"?formatBytes(totals.rx_bytes):formatBytes(totals.tx_bytes);
    setText(`activity-${key}`,value??0);
  }
  if(!points.length||!window.uPlot)return;
  const times=points.map(p=>p.time),requests=points.map(p=>p.requests),successes=points.map(p=>p.successes),failures=points.map(p=>p.failures),active=points.map(p=>p.active_jobs),rx=points.map(p=>p.rx_bytes),tx=points.map(p=>p.tx_bytes),c=chartTheme();
  const toolSeries=[{}, {label:"Requests",stroke:c.blue,width:2,points:{show:false}}, {label:"Success",stroke:c.accent,width:2,points:{show:false}}, {label:"Failed",stroke:c.red,width:2,points:{show:false}}];
  const trafficSeries=[{}, {label:"RX",stroke:c.violet,width:2,points:{show:false},value:(_,v)=>v==null?"—":formatBytes(v)}, {label:"TX",stroke:c.amber,width:2,points:{show:false},value:(_,v)=>v==null?"—":formatBytes(v)}];
  const jobsSeries=[{}, {label:"Active Jobs",stroke:c.amber,width:2,points:{show:false}}];
  upsertPlot("activity-tool","activity-tool-chart",[times,requests,successes,failures],toolSeries,285);
  upsertPlot("activity-jobs","activity-jobs-chart",[times,active],jobsSeries,285);
  upsertPlot("activity-traffic","activity-traffic-chart",[times,rx,tx],trafficSeries,285,formatBytes);
  setText("activity-updated",`更新于 ${formatTime(data.generated_at)} · ${data.bucket_seconds}s/点`);
}

export function resizePlots(){
  for(const plot of plots.values()){
    const host=plot.root?.parentElement;
    if(host&&host.clientWidth>0)plot.setSize({width:Math.max(280,Math.floor(host.clientWidth)),height:285});
  }
}
