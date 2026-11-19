import fs from 'node:fs';
const appearance=JSON.parse(fs.readFileSync(new URL('../assets/appearance.json',import.meta.url),'utf8'));
const aliases={bg:'canvas',panel:'panel',raised:'surface',panel2:'hover',line:'border',text:'text',muted:'text_dim',accent:'accent',secondary:'icon_yellow',green:'green',amber:'icon_yellow',red:'icon_red'};
let css='/* Generated from assets/appearance.json. ContactSolutions token provenance is recorded there. */\n';
for(const theme of ['dark','light']){
 css+=(theme==='dark'?':root,html[data-light=false]':'html[data-light=true]')+' {\n  color-scheme: '+theme+';\n';
 for(const [key,value]of Object.entries(appearance[theme]))css+=`  --fs-${key.replaceAll('_','-')}: ${value};\n`;
 for(const [key,value]of Object.entries(aliases))css+=`  --o-${key}: var(--fs-${value.replaceAll('_','-')});\n`;
 css+='  --o-shadow: 0 8px 28px var(--fs-shadow-window);\n}\n';
}
const target=new URL('../src/theme.css',import.meta.url);
if(process.argv.includes('--check')){if(fs.readFileSync(target,'utf8')!==css)throw Error('Regenerate src/theme.css with node scripts/generate-theme.mjs');console.log('Generated ContactSolutions theme matches token source.');}else{fs.writeFileSync(target,css);console.log('Generated ContactSolutions theme variables.');}
