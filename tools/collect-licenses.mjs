// Reproducible notices for the Windows dependency graph; no network license fetches.
import {readFileSync,readdirSync,writeFileSync,statSync} from 'node:fs';
import {dirname,join,resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
const root=resolve(import.meta.dirname,'..');
const metadata=spawnSync('cargo',['metadata','--locked','--format-version','1','--manifest-path','desktop/src-tauri/Cargo.toml','--features','tauri/custom-protocol','--filter-platform','x86_64-pc-windows-msvc'],{cwd:root,encoding:'utf8',maxBuffer:32*1024*1024});
if(metadata.status!==0)throw new Error(metadata.stderr);
const packages=JSON.parse(metadata.stdout).packages.filter(pkg=>pkg.source);
const overrides={
  'alloc-stdlib@0.3.0':['alloc-stdlib-0.3.0-BSD.txt'],
  'defmt-parser@1.0.0':['defmt-parser-1.0.0-MIT.txt','defmt-parser-1.0.0-APACHE.txt'],
  'selectors@0.38.0':['selectors-0.38.0-MPL.txt'],
  'tauri-plugin@2.7.2':['tauri-plugin-2.7.2-MIT.txt','tauri-plugin-2.7.2-APACHE.txt'],
  'webview2-com@0.39.1':['webview2-com-0.39.1-MIT.txt'],
  'webview2-com-macros@0.8.1':['webview2-com-macros-0.8.1-MIT.txt'],
  'webview2-com-sys@0.39.1':['webview2-com-0.39.1-MIT.txt'],
};
const compare=(a,b)=>a<b?-1:a>b?1:0;
const clean=text=>text.replace(/\r\n/g,'\n').replace(/[ \t]+$/gm,'').trim();
function documents(folder,explicit) {
  const names=readdirSync(folder).filter(name=>/^(licen[cs]e|copying|copyright|notice)/i.test(name)&&statSync(join(folder,name)).isFile());
  if(explicit&&!names.includes(explicit))names.push(explicit);
  return names.sort(compare).map(name=>({name,text:clean(readFileSync(join(folder,name),'utf8'))}));
}
const entries=[];
for(const pkg of packages) {
  let docs=documents(dirname(pkg.manifest_path),pkg.license_file);
  if(!docs.length)docs=(overrides[`${pkg.name}@${pkg.version}`]??[]).map(name=>({name,text:clean(readFileSync(join(root,'licenses/vendor',name),'utf8'))}));
  if(!docs.length)throw new Error(`License text missing: ${pkg.name} ${pkg.version}`);
  entries.push({name:`Rust: ${pkg.name} ${pkg.version}`,license:pkg.license??'See license text',source:pkg.repository??pkg.source,docs});
}
for(const name of ['@tauri-apps/api','vite']) {
  const folder=join(root,'desktop/node_modules',name);
  const pkg=JSON.parse(readFileSync(join(folder,'package.json'),'utf8'));
  const docs=documents(folder);
  if(!docs.length)throw new Error(`License text missing: ${name}`);
  entries.push({name:`JavaScript: ${name} ${pkg.version}`,license:pkg.license,source:typeof pkg.repository==='string'?pkg.repository:pkg.repository.url,docs});
}
entries.sort((a,b)=>compare(a.name,b.name));
const output=['Jack third-party dependency licenses',
  'Windows x64 dependency graph, including build and test dependencies. Not every listed component is included in the installed binary.',
  'Directly reused code, fonts and icons: see THIRD_PARTY_NOTICES.md. Upstream license text is reproduced below.',
  ...entries.map(entry=>['='.repeat(72),entry.name,`Declared license: ${entry.license}`,`Source: ${entry.source}`,
    ...entry.docs.map(doc=>`\n--- ${doc.name} ---\n${doc.text}`)].join('\n'))].join('\n\n')+'\n';
const destination=join(root,'THIRD_PARTY_LICENSES.txt');
if(process.argv.includes('--check')) {
  if(clean(readFileSync(destination,'utf8'))!==clean(output))throw new Error('Dependency license bundle is stale; run node tools/collect-licenses.mjs');
} else writeFileSync(destination,output);
console.log(`Dependency license texts: ${entries.length} packages verified.`);
