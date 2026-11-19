// Adapted from ContactSolutions src/chrome.rs (MIT, William Pyle 2026).
// Exact caption geometry/path construction; see THIRD_PARTY_NOTICES.md.
import {listen} from '@tauri-apps/api/event';
import './window-chrome.css';

type Rpc = <T=unknown>(action:string,payload?:unknown)=>Promise<T>;
type WindowState = {maximized:boolean;nativeFrame?:boolean};
let maximized=false;
let fallbackReported=false;
const svg=(body:string)=>`<svg class="caption-glyph" viewBox="0 0 14 14" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1">${body}</svg>`;
const minimize=svg('<path d="M2 7H12"/>');
const close=svg('<path d="M2 2L12 12M12 2L2 12"/>');
// egui's 10 x 10 Inside stroke maps to a 9 x 9 SVG centerline inset by .5.
const maximize=()=>svg(`${maximized?'<path d="M4 0H14V10"/>':''}<rect x="2.5" y="2.5" width="9" height="9"/>`);
export function windowTitle(mark:string,browser=true){return `<header class="window-title ${browser?'':'management-title'}" aria-label="FileSolutions window"><button data-window="systemMenu" class="window-icon" aria-label="Window menu (Alt+Space)" title="Window menu (Alt+Space)">${mark}</button>${browser?'<div id="window-tabs"></div>':'<span class="window-product">FileSolutions</span>'}<span class="window-drag"></span><div class="caption-controls"><button data-window="minimize" aria-label="Minimize window" title="Minimize window">${minimize}</button><button data-window="maximize" aria-label="${maximized?'Restore window':'Maximize window'}" title="${maximized?'Restore window':'Maximize window'}">${maximize()}</button><button data-window="close" class="window-close" aria-label="Close to tray" title="Close to tray (Alt+F4)">${close}</button></div></header>`;}

export function installWindowChrome(rpc:Rpc,notice:(message:string)=>void){
 const run=async(operation:string,extra:object={})=>{try{const state=await rpc<WindowState>('window',{operation,...extra});if(state)apply(state);}catch(e){notice(String(e));}};
 const apply=(state:WindowState)=>{maximized=state.maximized;document.documentElement.dataset.nativeFrame=String(!!state.nativeFrame);if(state.nativeFrame&&!fallbackReported){fallbackReported=true;notice('Custom window controls unavailable; using the Windows frame.');}const button=document.querySelector<HTMLButtonElement>('[data-window=maximize]');if(button){button.innerHTML=maximize();button.title=button.ariaLabel=maximized?'Restore window':'Maximize window';}document.documentElement.dataset.maximized=String(maximized);};
 const rect=(element:Element)=>{const r=element.getBoundingClientRect();return {left:r.left,top:r.top,right:r.right,bottom:r.bottom};};
 let scheduled=false;
 const layout=()=>{if(scheduled)return;scheduled=true;requestAnimationFrame(()=>{scheduled=false;const title=document.querySelector('.window-title');if(!title)return;void run('layout',{layout:{toolbar:rect(title),controls:[...title.querySelectorAll('.caption-controls button,.window-icon,#window-tabs')].map(rect),scale:devicePixelRatio}});});};
 const observer=new ResizeObserver(layout);
 let header:Element|null=null;
 new MutationObserver(()=>{const next=document.querySelector('.window-title');if(next!==header){if(header)observer.unobserve(header);header=next;if(header){observer.observe(header);void run('state');layout();}}}).observe(document.querySelector('#app')!,{childList:true});
 window.addEventListener('resize',layout);
 document.addEventListener('click',event=>{const b=(event.target as Element).closest<HTMLElement>('[data-window]');if(b)void run(b.dataset.window!);});
 // Wry enables WebView2 Settings9 nonclient regions. CSS app-region hands
 // caption input directly to Windows, without an asynchronous IPC drag race.
 document.addEventListener('contextmenu',event=>{const target=event.target as Element;if(!target.closest('.window-title')||target.closest('.caption-controls,#window-tabs'))return;event.preventDefault();void run('systemMenu');});
 document.addEventListener('keydown',event=>{if(event.altKey&&event.code==='Space'){event.preventDefault();void run('systemMenu');}});
 void listen<WindowState>('window-state',e=>apply(e.payload));
 void run('state');
}
