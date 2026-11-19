import {test,expect} from '@playwright/test';
import {browser} from './explorer-fixture';

test('independent tabs, back/up, sorting, selection and persisted paths',async({page})=>{
 await browser(page);await page.getByRole('button',{name:'New tab',exact:true}).click();
 await expect(page.getByRole('tab')).toHaveCount(2);
 await page.keyboard.press('Control+l');await page.getByRole('textbox',{name:'Address',exact:true}).fill('D:\\');await page.getByRole('textbox',{name:'Address',exact:true}).press('Enter');
 await page.getByRole('tab',{name:'C:\\Fixture',exact:true}).click();await expect(page.locator('[data-entry]')).toHaveCount(3);
 await page.getByRole('button',{name:'Name',exact:true}).click();await expect(page.locator('[data-entry]').nth(1)).toContainText('beta.txt');
 await page.locator('[data-entry]').nth(1).click();await page.locator('[data-entry]').nth(2).click({modifiers:['Control']});await expect(page.locator('[aria-selected=true][data-entry]')).toHaveCount(2);
 const states=await page.evaluate(()=>(window as any).__calls.filter((c:any)=>c.action==='saveBrowserState'));expect(states.at(-1).payload.tabs.map((t:any)=>t.path)).toEqual(['C:\\Fixture','D:\\']);
});
test('right click offers backup selection and delegates additional verbs to the actual shell IPC',async({page})=>{
 await browser(page);await page.getByText('alpha.txt',{exact:true}).click({button:'right'});
 await expect(page.getByRole('menuitem',{name:'Add to backup…',exact:true})).toBeVisible();
 await page.getByRole('menuitem',{name:'Show more options',exact:true}).click();
 expect(await page.evaluate(()=>(window as any).__calls.some((c:any)=>c.action==='shell'&&c.payload.verb==='context'))).toBe(true);
 await page.getByText('alpha.txt',{exact:true}).click({button:'right'});await page.getByRole('menuitem',{name:'Add to backup…',exact:true}).click();
 await page.locator('.backup-location-dialog [name=name]').fill('Fixture archive');await page.locator('.backup-location-dialog [name=path]').fill('E:\\Archive');await page.getByRole('button',{name:'Review changes',exact:true}).click();await expect(page.getByRole('button',{name:'Apply changes'})).toBeVisible();
 const payload=await page.evaluate(()=>(window as any).__calls.find((c:any)=>c.action==='backupSelectionPreview').payload);expect(payload).toMatchObject({path:'C:\\Fixture\\alpha.txt',newDestination:{name:'Fixture archive',path:'E:\\Archive'}});
});

test('clipboard paste and internal directory drag carry copy/move intent',async({page})=>{
 await browser(page);await page.getByRole('button',{name:'Paste (Ctrl+V)',exact:true}).click();
 expect(await page.evaluate(()=>(window as any).__calls.find((c:any)=>c.action==='transfer')?.payload.moving)).toBe(false);
 const source=await page.getByText('alpha.txt',{exact:true}).boundingBox(),target=await page.getByText('Photos',{exact:true}).boundingBox();
 await page.mouse.move(source!.x+30,source!.y+8);await page.mouse.down();await page.mouse.move(target!.x+30,target!.y+8,{steps:8});await page.mouse.up();
 await expect.poll(()=>page.evaluate(()=>(window as any).__calls.filter((c:any)=>c.action==='transfer').at(-1)?.payload.destination)).toBe('C:\\Fixture\\Photos');
 expect(await page.evaluate(()=>(window as any).__calls.filter((c:any)=>c.action==='transfer').at(-1)?.payload.moving)).toBe(true);
});
test('unavailable path preserves current folder and reports error',async({page})=>{
 await browser(page);await page.getByRole('textbox',{name:'Address',exact:true}).fill('C:\\Unavailable');await page.getByRole('textbox',{name:'Address',exact:true}).press('Enter');await expect(page.locator('#toast')).toContainText('Folder unavailable');await expect(page.getByRole('textbox',{name:'Address',exact:true})).toHaveValue('C:\\Fixture');
});
test('keyboard commands retain focus after tab and list rebuilds',async({page})=>{
 await browser(page);await page.getByRole('button',{name:'New tab',exact:true}).click();
 await expect(page.getByRole('tab')).toHaveCount(2);
 await page.keyboard.press('Control+l');
 const address=page.getByRole('textbox',{name:'Address',exact:true});await expect(address).toBeFocused();
 await address.press('Enter');await expect(page.locator('#file-browser')).toHaveAttribute('aria-busy','false');
 await page.keyboard.press('Control+a');await expect(page.locator('[data-entry][aria-selected=true]')).toHaveCount(3);
 await page.keyboard.press('Control+c');
 await expect.poll(()=>page.evaluate(()=>(window as any).__calls.filter((c:any)=>c.action==='shell'&&c.payload.verb==='copy').at(-1)?.payload.paths.length)).toBe(3);
});
test('system theme changes while browsing preserve tabs and selection',async({page})=>{
 await page.emulateMedia({colorScheme:'dark'});await browser(page,'system');await page.getByRole('button',{name:'New tab',exact:true}).click();await page.getByText('alpha.txt',{exact:true}).click();
 await page.emulateMedia({colorScheme:'light'});await expect(page.locator('html')).toHaveAttribute('data-light','true');await expect(page.getByRole('tab')).toHaveCount(2);await expect(page.locator('[data-entry][aria-selected=true]')).toContainText('alpha.txt');
 await page.emulateMedia({colorScheme:'dark'});await expect(page.locator('html')).toHaveAttribute('data-light','false');await expect(page.getByRole('textbox',{name:'Address',exact:true})).toHaveValue('C:\\Fixture');await expect(page.locator('[data-entry][aria-selected=true]')).toContainText('alpha.txt');
});
for(const width of [780,1280])test(`browser is usable at ${width}px with screenshot`,async({page})=>{await page.setViewportSize({width,height:850});await browser(page);await expect(page.getByRole('button',{name:'Minimize window',exact:true})).toBeVisible();await expect(page.locator('.file-status')).toBeVisible();expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);await page.screenshot({path:`test-results/filesolutions-browser-${width}.png`});});

test('all view modes persist and preserve attached icon status and selection',async({page})=>{
 await browser(page);await page.getByText('alpha.txt',{exact:true}).click();
 for(const [id,label] of Object.entries({'extra-large':'Extra large icons',large:'Large icons',medium:'Medium icons',small:'Small icons',list:'List',details:'Details',tiles:'Tiles',content:'Content'})){
  await page.getByRole('button',{name:'View',exact:true}).click();await page.getByRole('menuitemradio',{name:label,exact:true}).click();await expect(page.locator('.file-list')).toHaveClass(new RegExp(id));await expect(page.locator('[data-entry][aria-selected=true]')).toContainText('alpha.txt');expect(await page.evaluate(()=>(window as any).__calls.filter((c:any)=>c.action==='saveBrowserState').at(-1).payload.tabs[0].view)).toBe(id);
 }
});
test('column chooser resolves inherited and multiple assignments and persists widths',async({page})=>{
 await browser(page);await page.evaluate(()=>{const c=(window as any).__browserConfig;c.destinations=[{id:'a',name:'Archive A',path:'D:\\FixtureArchiveA'},{id:'b',name:'Archive B',path:'E:\\B'}];c.roots=[{id:'a',path:'C:\\Fixture\\Photos\\A',enabled:true},{id:'b',path:'C:\\Fixture\\Photos\\B',enabled:true}];c.assignments={a:'a',b:'b'};});
 await page.locator('th').first().click({button:'right'});await page.getByRole('menuitemcheckbox',{name:'Backup folder',exact:true}).click();await expect(page.locator('[data-entry]').first().locator('.cell-destination')).toContainText('Multiple locations: Archive A (D:\\FixtureArchiveA); Archive B (E:\\B)');
 const resize=page.getByRole('separator',{name:'Resize Backup folder column',exact:true});await resize.focus();await page.keyboard.press('ArrowRight');expect(await page.evaluate(()=>(window as any).__calls.filter((c:any)=>c.action==='saveBrowserState').at(-1).payload.preferences.widths.destination)).toBe(250);
});
test('double click opens folders and caption tabs remain interactive',async({page})=>{
 await browser(page);await page.getByText('Photos',{exact:true}).dblclick();await expect(page.locator('#file-address')).toHaveValue('C:\\Fixture\\Photos');await page.keyboard.press('Alt+ArrowLeft');await expect(page.locator('[data-entry]')).toHaveCount(3);await page.getByRole('button',{name:'New tab',exact:true}).click();await expect(page.getByRole('tab')).toHaveCount(2);await page.getByRole('button',{name:'Close tab',exact:true}).last().click();await expect(page.getByRole('tab')).toHaveCount(1);
});

test('drive labels stay compact and long breadcrumbs keep the current folder visible after resize',async({page})=>{
 await browser(page);await page.getByRole('button',{name:'This PC',exact:true}).last().click();await expect(page.locator('.drive-grid button>span').first()).toHaveCSS('font-size','14px');
 const longPath='C:\\Fixture\\Long parent folder\\Another long folder\\A third long folder\\CurrentFolder';await page.keyboard.press('Control+l');await page.locator('#file-address').fill(longPath);await page.locator('#file-address').press('Enter');await expect(page.locator('#file-browser')).toHaveAttribute('aria-busy','false');
 for(const width of [1800,780]){await page.setViewportSize({width,height:850});await expect.poll(()=>page.locator('.breadcrumbs').evaluate(el=>Math.abs(el.scrollWidth-el.clientWidth-el.scrollLeft))).toBeLessThan(2);}
});
