import paths from '../assets/ui-icons.json';
import appearance from '../assets/appearance.json';

const glyphs:Record<string,string>={
 back:'M15 4 7 12l8 8M7 12h15',forward:'m9 4 8 8-8 8M2 12h15',up:'m4 12 8-8 8 8M12 4v18',refresh:'M20 8A8 8 0 1 0 21 15M20 2v6h-6',
 plus:'M12 5v14M5 12h14',close:'m6 6 12 12M18 6 6 18',chevron:'m9 5 7 7-7 7',down:'m6 9 6 6 6-6',
 new:'M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0M12 7v10M7 12h10',cut:'m8 9 10 11M8 15 18 3M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6M6 3a3 3 0 1 0 0 6 3 3 0 0 0 0-6',
 copy:'M8 6V3h12v14h-3M15 7H4v14h11z',paste:'M8 4H5v18h15V4h-3M9 2h7v5H9z',rename:'M2 6h9v12H2zM16 3v18M13 3h6M13 21h6M19 6h3v12h-3',
 share:'M13 5 20 10l-7 5v-4c-5 0-7 2-9 5 0-7 3-10 9-10z',delete:'M3 6h18M9 3h6M6 6l1 15h10l1-15M10 10v7M14 10v7',
 sort:'M5 3v18m-3-3 3 3 3-3M13 4h8M13 9h6M13 14h4M13 19h2',view:'M3 5h18M3 10h18M3 15h18M3 20h18',more:'M5 12h.01M12 12h.01M19 12h.01',preview:'M3 4h18v16H3zM15 4v16',
 search:'M16 10a6 6 0 1 1-12 0 6 6 0 0 1 12 0m-1 5 6 6',pc:'M3 3h18v14H3zM12 17v4M8 21h8',network:'M8 3h8v6H8zM2 16h7v6H2zM15 16h7v6h-7zM12 9v4M5 16v-3h14v3',
 desktop:'M2 3h20v14H2zM12 17v4M7 21h10',download:'M12 2v14m-6-6 6 6 6-6M3 18v4h18v-4',documents:'M5 2h10l4 4v16H5zM14 2v5h5M8 11h8M8 15h8M8 18h6',pictures:'M3 3h18v18H3zM3 17l6-7 5 5 3-3 4 5M16 6h.01',music:'M9 18V5l11-3v14M9 7l11-3M9 18a3 3 0 1 1-3-3M20 16a3 3 0 1 1-3-3',videos:'M3 3h18v18H3zM3 8h18M3 16h18M8 3v18M16 3v18',home:'m2 11 10-9 10 9M5 9v12h5v-7h4v7h5V9',gallery:'M3 5h18v16H3zM6 2h12M3 18l5-6 4 4 3-3 6 6M16 9h.01',pin:'m14 2 8 8-4 1-4 5-3-3-7 7 7-7-3-3 5-4z',drive:'M3 9h18l2 11H1zM5 4h14l2 5M5 16h.01M9 16h.01',check:'m5 12 5 5L20 6',folder:'M2 6h8l2 2h10v13H2z',grid:'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z',info:'M12 10v7M12 6h.01M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0',backup:paths.paths.shield,
};
export function icon(name:string){return `<svg class="explorer-icon icon-${name}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${glyphs[name]||glyphs.documents}"/></svg>`;}
export function shortcutIcon(name:string){
 const bodies:Record<string,string>={
 home:'<path fill="#e8e8e8" d="M5 10h14v11H5z"/><path fill="#6b7d88" d="M9 14h6v7H9z"/><path fill="none" stroke="#f49b18" stroke-width="2.5" stroke-linecap="round" d="m2 11 10-9 10 9"/>',
 desktop:'<rect x="2" y="3" width="20" height="15" rx="1" fill="#389fc5"/><path fill="#a2edfa" d="M3 4h18v5H3z"/><path fill="#648797" d="M10 18h4v3h4v1H6v-1h4z"/>',
 documents:'<path fill="#f8f8f8" d="M6 2h8l5 5v15H6z"/><path fill="#a7c5df" d="M14 2v6h5z"/><path stroke="#4482ab" d="M8 12h9M8 15h9M8 18h6"/>',
 pictures:'<rect x="2" y="2" width="20" height="20" rx="2" fill="#1e9ccc"/><circle cx="16" cy="7" r="2" fill="#fff"/><path fill="#d7f6ff" d="m3 20 6-10 5 6 3-3 4 7z"/>',
 gallery:'<rect x="3" y="2" width="18" height="20" rx="2" fill="#2596cf"/><circle cx="15" cy="7" r="2" fill="#fff"/><path fill="#b8eaff" d="m3 20 6-9 6 9z"/><path fill="#83b4cc" d="m11 20 5-7 5 7z"/>',
 music:'<circle cx="12" cy="12" r="11" fill="#d86e85"/><path fill="none" stroke="#fff" stroke-width="1.5" d="M10 16V6l7-2v10M10 8l7-2"/><ellipse cx="8" cy="17" rx="3" ry="2" fill="#fff"/><ellipse cx="15" cy="15" rx="3" ry="2" fill="#fff"/>',
 videos:'<rect x="3" y="2" width="18" height="20" rx="2" fill="#8d55cd"/><path fill="#ede2fc" d="m9 7 8 5-8 5z"/><path stroke="#d8c7ee" d="M4 5h2M4 10h2M4 15h2M4 20h2"/>'
 };return bodies[name]?`<svg class="explorer-icon icon-${name}" viewBox="0 0 24 24" aria-hidden="true">${bodies[name]}</svg>`:icon(name);
}
export function fileGlyph(directory:boolean,name=''){
 if(directory)return '<svg class="file-base" viewBox="0 0 32 32" aria-hidden="true"><path fill="#E8AD21" d="M2 7a2 2 0 0 1 2-2h8l3 3h13a2 2 0 0 1 2 2v15H2z"/><path fill="#FFDA63" d="M2 11h28v14a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2z"/><path fill="#F3C344" d="M2 13h28v12H2z"/></svg>';
 const ext=name.split('.').at(-1)?.toLowerCase(),picture=['png','jpg','jpeg','webp','gif','svg'].includes(ext||'');
 return `<svg class="file-base" viewBox="0 0 32 32" aria-hidden="true"><path fill="#FAFAFA" stroke="#A2A2A2" d="M7 2h12l7 7v21H7z"/><path fill="#D0D0D0" d="M19 2v8h7z"/>${picture?'<path fill="#40B2DD" d="M9 13h15v13H9z"/><path fill="#DDF6FF" d="m9 25 5-8 4 5 3-4 3 7z"/>':['txt','md','json'].includes(ext||'')?'<path stroke="#8D969B" d="M11 15h11M11 19h11M11 23h8"/>':''}</svg>`;
}
// Reuse the same glyphs/colors as generated Windows status overlays, on each icon.
export function overlay(status:string){const names:Record<string,[string,string]>={backed_up:['check',appearance.dark.icon_green],included:['clock',appearance.dark.icon_yellow],pending:['clock',appearance.dark.icon_yellow],syncing:['sync',appearance.dark.accent],excluded:['minus',appearance.dark.text_dim],issue:['issue',appearance.dark.icon_red],paused:['pause',appearance.dark.icon_yellow],offline:['offline',appearance.dark.text_dim]};const item=names[status];if(!item)return '';return `<svg class="file-overlay" viewBox="0 0 32 32" aria-hidden="true"><circle cx="16" cy="16" r="15" fill="${item[1]}" stroke="${appearance.dark.canvas}" stroke-width="1.5"/><g transform="translate(4 4)" fill="none" stroke="${appearance.dark.canvas}" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round"><path d="${(paths.paths as Record<string,string>)[item[0]]}"/></g></svg>`;}
