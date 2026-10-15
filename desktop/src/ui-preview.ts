import './ui-preview.css';

const options=document.querySelector<HTMLFormElement>('#preview-options')!;
const preview=document.querySelector<HTMLIFrameElement>('#preview')!;
const open=document.querySelector<HTMLAnchorElement>('#open-preview')!;
const initial=new URLSearchParams(location.search);
for(const input of options.elements) {
  if(input instanceof HTMLSelectElement && initial.has(input.name))input.value=initial.get(input.name)!;
  if(input instanceof HTMLInputElement && input.type==='checkbox')input.checked=initial.has(input.name);
}
function refreshPreview() {
  const values=new FormData(options);
  const selection=new URLSearchParams();
  for(const [key,value] of values)selection.set(key,String(value));
  history.replaceState(null,'',`?${selection}`);
  const query=new URLSearchParams({fixture:'design',sonar:'',interactive:'', 'grouped-apps':'',theme:String(values.get('theme')),lang:String(values.get('lang')),groups:String(values.get('groups'))});
  const screen=values.get('screen');
  if(screen==='tray')query.set('view','tray');else query.set('tab',String(screen));
  const scenario=String(values.get('scenario'));
  if(scenario!=='connected')query.set(scenario,'');
  if(scenario==='missing-source'){query.set('groups','4');query.set('missing-source','aux');}
  if(values.has('expanded'))query.set('expanded','');
  const url=`/?${query}`;
  preview.style.width=screen==='tray'?'360px':'400px';
  preview.style.height=screen==='tray'?'760px':'680px';
  preview.src=url;open.href=url;
}
options.addEventListener('submit',event=>event.preventDefault());
options.addEventListener('change',refreshPreview);
document.getElementById('reload-preview')!.addEventListener('click',()=>{preview.src=open.href;});
refreshPreview();
