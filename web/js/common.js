export const $=id=>document.getElementById(id);

const dateTime=new Intl.DateTimeFormat("zh-CN",{year:"numeric",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",second:"2-digit",hour12:false});
const timeOnly=new Intl.DateTimeFormat("zh-CN",{hour:"2-digit",minute:"2-digit",second:"2-digit",hour12:false});

export const setText=(id,value)=>{const el=$(id);if(el)el.textContent=value??"—";};
export const formatTime=seconds=>dateTime.format(new Date(seconds*1000));
export const formatShortTime=seconds=>timeOnly.format(new Date(seconds*1000));

export function formatUptime(seconds){
  const days=Math.floor(seconds/86400),hours=Math.floor(seconds%86400/3600),minutes=Math.floor(seconds%3600/60),secs=seconds%60;
  return (days?`${days}天 `:"")+`${String(hours).padStart(2,"0")}:${String(minutes).padStart(2,"0")}:${String(secs).padStart(2,"0")}`;
}

export function formatBytes(value){
  let n=Number(value)||0;const units=["B","KiB","MiB","GiB","TiB"];let i=0;
  while(n>=1024&&i<units.length-1){n/=1024;i++;}
  return `${n>=100||i===0?n.toFixed(0):n>=10?n.toFixed(1):n.toFixed(2)} ${units[i]}`;
}

export function node(tag,className,text){
  const el=document.createElement(tag);
  if(className)el.className=className;
  if(text!==undefined)el.textContent=text;
  return el;
}

export function outcomeClass(outcome){
  return ["succeeded","running","accepted"].includes(outcome)?"good":["failed","timed_out","cancelled","interrupted"].includes(outcome)?"bad":"neutral";
}

export function displayValue(value){
  if(Array.isArray(value))return value.length?value.join(", "):"—";
  if(typeof value==="boolean")return value?"启用":"禁用";
  if(value===null||value===undefined||value==="")return"—";
  return String(value);
}
