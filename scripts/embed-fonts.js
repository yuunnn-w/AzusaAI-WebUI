/* 把 KaTeX 的 woff2 字体转成 base64 内联进 CSS —— node scripts/embed-fonts.js
   读 src/katex.min.css + src/fonts/,输出 src/katex-embedded.css */
const fs = require('fs');
const path = require('path');
const SRC = path.join(__dirname, '..', 'src');
let css = fs.readFileSync(path.join(SRC, 'katex.min.css'), 'utf8');
const before = (css.match(/fonts\//g) || []).length;
css = css.replace(
  /url\(fonts\/([A-Za-z0-9_-]+\.woff2)\)\s*format\("woff2"\)\s*,\s*url\(fonts\/[A-Za-z0-9_-]+\.woff\)\s*format\("woff"\)\s*,\s*url\(fonts\/[A-Za-z0-9_-]+\.ttf\)\s*format\("truetype"\)/g,
  (_m, f) => 'url(data:font/woff2;base64,' + fs.readFileSync(path.join(SRC, 'fonts', f)).toString('base64') + ') format("woff2")'
);
css = css.replace(/\/\*# sourceMappingURL=[^*]*\*\//g, '');
css = css.replace(/\s*\/\*[^*]*\*\/\s*(?=@font-face)/g, '\n');
const after = (css.match(/fonts\//g) || []).length;
fs.writeFileSync(path.join(SRC, 'katex-embedded.css'), css);
console.log('font refs before:', before, ' after:', after, ' css bytes:', css.length);
