import {test,expect,type Page} from '@playwright/test';
import fs from 'node:fs';
const appearance=JSON.parse(fs.readFileSync('assets/appearance.json','utf8'));
const rgb=(hex:string)=>`rgb(${[1,3,5].map(i=>parseInt(hex.slice(i,i+2),16)).join(', ')})`;
async function fixture(page:Page,theme:string){
 await page.addInitScript(theme=>{
  const w=window as any;w.isTauri=true;w.__chromeCalls=[];w.__maximized=false;w.__fail=false;w.__fallback=false;
  const callbacks=new Map(),events=new Map();let next=1;
  w.__windowEvent=(maximized:boolean)=>{w.__maximized=maximized;callbacks.get(events.get('window-state'))?.({payload:{maximized}});};
  w.__TAURI_INTERNALS__={transformCallback:(fn:Function)=>{const id=next++;callbacks.set(id,fn);return id;},unregisterCallback:(id:number)=>callbacks.delete(id),invoke:async(name:string,args:any)=>{
   if(name==='plugin:event|listen'){events.set(args.event,args.handler);return 1;}if(name==='plugin:event|unlisten')return;
   const {action,payload}=args;
   if(action==='window'){w.__chromeCalls.push(payload);if(w.__fail)throw Error('Window fixture failure');if(payload.operation==='maximize')w.__maximized=!w.__maximized;return {maximized:w.__maximized,nativeFrame:w.__fallback};}
   if(action==='snapshot')return {config:{revision:1,deviceId:'chrome-fixture',roots:[],rules:[],theme,continuous:false,startAtSignIn:false,destinations:[],assignments:{},pausedUntil:null},progress:{phase:'idle',message:'Fixture',currentFile:'',sent:0,total:0,lastBackup:null},counts:{backedUp:0,pending:0,excluded:0,issues:0,retained:0},recent:[],version:'0.3.0'};
   if(action==='browserLocations')return [];if(action==='shellIcons')return {};if(action==='browse')return {path:'',parent:null,entries:[],drives:['C:\\']};
   if(['browserState','intent','checkUpdate'].includes(action))return null;
   if(action==='saveBrowserState')return;
   throw Error('Unexpected fixture command '+action);
  }};
 },theme);
 await page.goto('/');await expect(page.locator('#file-browser')).toHaveAttribute('aria-busy','false');
 await expect.poll(()=>page.evaluate(()=>(window as any).__chromeCalls.some((x:any)=>x.operation==='layout'))).toBe(true);
}
for(const theme of ['light','dark'])test(`Explorer caption geometry and states: ${theme}`,async({browser})=>{
 const context=await browser.newContext({viewport:{width:1220,height:900},deviceScaleFactor:1.25});const page=await context.newPage();
 await fixture(page,theme);const t=theme==='dark'?{title_bar:'#090909',hover:'#303030',close_hover:'#C42B1C',text:'#F5F5F5'}:{title_bar:'#F3F3F3',hover:'#EDEDED',close_hover:'#C42B1C',text:'#191919'};
 const title=page.locator('.window-title'),buttons=page.locator('.caption-controls button');
 expect((await title.boundingBox())!.height).toBe(40);
 for(let i=0;i<3;i++){const r=(await buttons.nth(i).boundingBox())!;expect(r.width).toBe(46);expect(r.height).toBe(32);expect(r.x).toBe(1220-138+i*46);}
 expect(await page.locator('.window-icon .mark').boundingBox()).toMatchObject({x:12,y:12,width:16,height:16});
 await expect(title).toHaveCSS('background-color',rgb(t.title_bar));
 for(let i=0;i<3;i++){
  const b=buttons.nth(i);await b.hover();await expect(b).toHaveCSS('background-color',rgb(i===2?t.close_hover:t.hover));await expect(b).toHaveCSS('color',rgb(i===2?'#FFFFFF':t.text));
  await page.mouse.down();await expect(b).toHaveCSS('background-color',rgb(i===2?t.close_hover:t.hover));await page.mouse.move(250,150);await page.mouse.up();
 }
 await page.mouse.move(250,150);await page.screenshot({path:`test-results/chrome-${theme}-normal.png`});await title.screenshot({path:`test-results/chrome-${theme}-normal-title.png`});
 await page.getByRole('button',{name:'Maximize window',exact:true}).click();await expect(page.getByRole('button',{name:'Restore window',exact:true})).toBeVisible();await expect(page.locator('[data-window=maximize] path')).toHaveAttribute('d','M4 0H14V10');
 await page.mouse.move(250,150);await page.screenshot({path:`test-results/chrome-${theme}-maximized.png`});await title.screenshot({path:`test-results/chrome-${theme}-maximized-title.png`});
 // OS-originated maximize/restore events must update labels and glyphs too.
 await page.evaluate(()=>(window as any).__windowEvent(false));await expect(page.getByRole('button',{name:'Maximize window',exact:true})).toBeVisible();await expect(page.locator('[data-window=maximize] path')).toHaveCount(0);
 await page.getByRole('button',{name:'Minimize window',exact:true}).focus();await expect(page.locator('[data-window=minimize]')).toHaveCSS('background-color',rgb(t.hover));await page.keyboard.press('Enter');
 await page.keyboard.press('Tab');await page.keyboard.press('Space');await expect(page.getByRole('button',{name:'Restore window',exact:true})).toBeFocused();
 await page.getByRole('button',{name:'Window menu (Alt+Space)',exact:true}).click({button:'right'});await page.keyboard.press('Alt+Space');
 await page.locator('.window-drag').dispatchEvent('mousedown',{button:0,clientX:200,clientY:24,detail:2});
 const calls=await page.evaluate(()=>(window as any).__chromeCalls);expect(calls.filter((x:any)=>x.operation==='systemMenu')).toHaveLength(2);expect(calls.some((x:any)=>x.operation==='minimize')).toBe(true);expect(calls.some((x:any)=>x.operation==='captionPointer')).toBe(false);await expect(title).toHaveCSS('app-region','drag');await expect(buttons.first()).toHaveCSS('app-region','no-drag');
 const layout=calls.find((x:any)=>x.operation==='layout').layout;expect(layout.scale).toBe(1.25);expect(layout.controls).toHaveLength(5);expect(layout.toolbar.bottom).toBe(40);
 await page.evaluate(()=>(window as any).__fail=true);await page.getByRole('button',{name:'Close to tray'}).click();await expect(page.locator('#toast')).toContainText('Window fixture failure');
 await page.evaluate(()=>{(window as any).__fail=false;(window as any).__fallback=true;});await page.getByRole('button',{name:'Close to tray'}).click();await expect(title).toBeHidden();await expect(page.locator('#toast')).toContainText('using the Windows frame');
 fs.writeFileSync(`test-results/chrome-${theme}-checks.json`,JSON.stringify({theme,dpi:120,scope:'Edge IPC fixture; native window input is not simulated as acceptance',geometry:{titleHeight:40,buttonWidth:46,glyphSize:10,iconSize:16},states:['normal','maximized','hover','pressed','keyboard focus','native event update','error','fallback'],calls},null,2));await context.close();
});
