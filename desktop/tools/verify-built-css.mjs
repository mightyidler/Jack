import {readFileSync,readdirSync} from 'node:fs';
import {cssTimeMs} from '../src/motion-time.ts';

// Vite 8's minifier can discard Chromium's standard property when the prefixed
// declaration follows it. Check the shipped CSS, not just the source stylesheet.
const directory = new URL('../dist/assets/',import.meta.url);
const css = readdirSync(directory).filter(file=>file.endsWith('.css'))
  .map(file=>readFileSync(new URL(file,directory),'utf8')).join('\n');
// The tray panel has an opaque background and does not use backdrop blur.
for (const selector of ['.header','.sonar-output-menu','.confirm-dialog']) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g,'\\$&');
  const blocks = [...css.matchAll(new RegExp(`${escaped}\\{([^}]+)\\}`,'g'))];
  if (!blocks.some(match=>/(?:^|;)backdrop-filter:(?!none(?:;|$))/.test(match[1]))) {
    throw new Error(`Built CSS lost the standard backdrop-filter for ${selector}`);
  }
}
console.log('Built CSS: Chromium backdrop-filter preserved for header, menus and dialog.');
const toast = [...css.matchAll(/\.toast\{([^}]+)\}/g)].find(match=>match[1].includes('--toast-visible-time'));
if (!toast || cssTimeMs(toast[1].match(/--toast-visible-time:([^;}]+)/)?.[1]??'',NaN)!==3000) {
  throw new Error('Built CSS toast lifetime must resolve to 3000 milliseconds.');
}
if(!/transition:opacity (?:\.2s|200ms),transform (?:\.2s|200ms)/.test(toast[1])) {
  throw new Error('Built CSS must preserve Bald toast transition timing.');
}
console.log('Built CSS: Bald toast transition 200ms and lifetime 3000ms preserved.');
const dividerDuration=css.match(/--divider-fade-duration:([^;}]+)/)?.[1];
if(cssTimeMs(dividerDuration??'',NaN)!==250) {
  throw new Error('Built CSS divider fade must resolve to 250 milliseconds.');
}
const dividers=[...css.matchAll(/\.device-divider\.hidden\{([^}]+)\}/g)];
if(!dividers.some(match=>/(?:^|;)opacity:0(?:;|$)/.test(match[1]))) {
  throw new Error('Built CSS must hide device dividers using opacity.');
}
console.log('Built CSS: divider opacity fade 250ms preserved.');
for(const [token,expected] of [['--device-text-duration',250],['--device-surface-duration',250],['--volume-settle-time',380],['--volume-status-duration',150],['--volume-number-duration',150]]) {
  const value=css.match(new RegExp(`${token}:([^;}]+)`))?.[1];
  if(cssTimeMs(value??'',NaN)!==expected)throw new Error(`Built CSS ${token} must resolve to ${expected} milliseconds.`);
}
console.log('Built CSS: device text/surface transitions, volume settle and status swap timing preserved.');
const chevronDuration=css.match(/--acc-chevron:([^;}]+)/)?.[1];
if(cssTimeMs(chevronDuration??'',NaN)!==250 || !/\.t-acc\[data-open=true\] \.t-acc-chevron\{transform:rotate\(180deg\)/.test(css)) {
  throw new Error('Built CSS input chevron must rotate 180 degrees over 250 milliseconds.');
}
console.log(`Built CSS: input chevron ${chevronDuration} resolves to 250ms; device swap uses ${css.match(/--device-text-duration:([^;}]+)/)?.[1]} for both layers.`);
