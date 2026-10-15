import {readFileSync,readdirSync} from 'node:fs';

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
