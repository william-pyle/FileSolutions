import {test,expect} from '@playwright/test';
import {browser as fixture} from './explorer-fixture';
const rgb=(hex:string)=>`rgb(${[1,3,5].map(i=>parseInt(hex.slice(i,i+2),16)).join(', ')})`;
function contrast(a:string,b:string){const light=(s:string)=>{const c=s.match(/[\d.]+/g)!.slice(0,3).map(Number).map(x=>{x/=255;return x<=.04045?x/12.92:((x+.055)/1.055)**2.4;});return c[0]*.2126+c[1]*.7152+c[2]*.0722;};const x=light(a),y=light(b);return (Math.max(x,y)+.05)/(Math.min(x,y)+.05);}
for(const theme of ['light','dark'])for(const width of [780,1420])test(`approved Explorer surfaces and readable states: ${theme} ${width}`,async({page})=>{
 await page.setViewportSize({width,height:940});await fixture(page,theme);const dark=theme==='dark',canvas=rgb(dark?'#191919':'#FFFFFF'),panel=rgb(dark?'#202020':'#FAFAFA');
 await expect(page.locator('.window-title')).toHaveCSS('background-color',rgb(dark?'#090909':'#F3F3F3'));
 await expect(page.locator('#file-browser')).toHaveCSS('background-color',canvas);await expect(page.locator('.preview-pane')).toHaveCSS('background-color',panel);
 const readable=async(selector:string,bg:string)=>{const fg=await page.locator(selector).first().evaluate(el=>getComputedStyle(el).color);expect(contrast(fg,bg),selector+' contrast').toBeGreaterThanOrEqual(4.5);};
 await readable('.file-name',canvas);await readable('.file-status',panel);
 await page.getByText('alpha.txt',{exact:true}).click();const selected=rgb(dark?'#505050':'#E5E5E5');await expect(page.locator('[data-entry][aria-selected=true]')).toHaveCSS('background-color',selected);await readable('[aria-selected=true] .file-name',selected);
 await page.getByRole('button',{name:'View',exact:true}).click();await expect(page.locator('.explorer-popup')).toHaveCSS('background-color',panel);await readable('.explorer-popup button',panel);await page.keyboard.press('Escape');
 await page.getByRole('button',{name:'Add backup location',exact:true}).click();const field=rgb(dark?'#2B2B2B':'#F1F1F1');await expect(page.locator('.backup-location-dialog input').first()).toHaveCSS('background-color',field);await readable('.backup-location-dialog input',field);await readable('.backup-location-dialog .primary',rgb('#0067C0'));await page.getByRole('button',{name:'Cancel',exact:true}).click();
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);await expect(page.locator('.file-status')).toBeVisible();await page.screenshot({path:`test-results/explorer-${theme}-${width}.png`});
});
test('backup overlays stay attached in all eight modes with accessible status',async({page})=>{
 await fixture(page);await page.evaluate(()=>{const w=window as any;w.__entries=[{name:'alpha.txt',path:'C:\\Fixture\\alpha.txt',directory:false,size:42,modified:1800000000,kind:'TXT',backup:'Included',status:'backed_up',reason:'Verified fixture copy',destination:'Archive'}];});await page.getByRole('button',{name:'Refresh (F5)',exact:true}).click();
 for(const label of ['Extra large icons','Large icons','Medium icons','Small icons','List','Details','Tiles','Content']){await page.getByRole('button',{name:'View',exact:true}).click();await page.getByRole('menuitemradio',{name:label,exact:true}).click();const image=page.getByRole('img',{name:'File, Backed up',exact:true});await expect(image).toBeVisible();await expect(image.locator('.file-overlay')).toHaveCount(1);const a=(await image.boundingBox())!,b=(await image.locator('.file-overlay').boundingBox())!;expect(b.x+b.width).toBeGreaterThan(a.x);expect(b.x).toBeLessThan(a.x+a.width);expect(b.y+b.height).toBeGreaterThan(a.y);expect(b.y).toBeLessThan(a.y+a.height);}
});
