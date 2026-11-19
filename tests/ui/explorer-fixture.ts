import {expect,type Page} from '@playwright/test';

// Explicit IPC fixture. Native shell and filesystem checks are separate.
export async function browser(page:Page,theme='dark'){
 await page.addInitScript(theme=>{
  const config={revision:1,deviceId:'fixture',roots:[],rules:[],pausedUntil:null,continuous:false,pauseOnMetered:false,priorVersions:10,stagingBudgetBytes:2**30,freeSpaceReserveBytes:2**30,uploadLimitKib:null,startAtSignIn:false,theme,destination:null,destinations:[],assignments:{}};
  const entry=(name:string,directory=false)=>({name,path:`C:\\Fixture\\${name}`,directory,size:42,modified:1800000000,kind:directory?'Folder':'TXT',backup:'Not selected',reason:'No backup source includes this item',destination:''});
  let saved:any=null;(window as any).__browserConfig=config;(window as any).__calls=[];(window as any).isTauri=true;
  (window as any).__TAURI_INTERNALS__={transformCallback:()=>1,unregisterCallback:()=>{},invoke:async(name:string,args:any)=>{
   if(name.startsWith('plugin:event|'))return 1;
   const {action,payload}=args;(window as any).__calls.push({action,payload});
   switch(action){
    case 'snapshot':return {config,progress:{phase:'idle',message:'Fixture',currentFile:'',sent:0,total:0,lastBackup:null},counts:{backedUp:0,pending:0,excluded:0,issues:0,retained:0},recent:[],version:'0.3.0'};
    case 'intent':case 'checkUpdate':return null;
    case 'browserState':return saved;
    case 'saveBrowserState':saved=structuredClone(payload);return;
    case 'browse':if(payload.path==='C:\\Unavailable')throw Error('Folder unavailable');return {path:payload.path,parent:payload.path==='C:\\Fixture'?'C:\\':null,entries:payload.path==='C:\\Fixture'?(window as any).__entries??[entry('Photos',true),entry('alpha.txt'),entry('beta.txt')]:[],drives:['C:\\','D:\\']};
    case 'browserLocations':return [{name:'Documents',icon:'documents',path:'C:\\Fixture'}];case 'shellIcons':return {};case 'pickDestination':return 'E:\\Archive';case 'backupSelectionPreview':case 'shellPreview':case 'inheritPreview':return {token:'review',revision:1,description:'Backup selection review',included:1,excluded:0,bytes:42,examples:[],warnings:[]};
    case 'shell':if(payload.verb==='clipboard')return {paths:['C:\\Fixture\\alpha.txt'],moving:false};return;
    case 'transfer':case 'window':case 'newFolder':case 'rename':return;
    default:throw Error(`Unexpected test command: ${action}`);
   }
  }};
 },theme);
 await page.goto('/');await expect(page.getByRole('heading',{name:'Devices and drives'})).toBeVisible();
 await page.keyboard.press('Control+l');const address=page.getByRole('textbox',{name:'Address',exact:true});await expect(address).toBeFocused({timeout:1000});await address.fill('C:\\Fixture');await address.press('Enter');
 await expect(page.getByRole('table',{name:'Folder contents'})).toBeVisible();
}
