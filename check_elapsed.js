const fs = require('fs');
const path = 'dist/main.js';
let c = fs.readFileSync(path, 'utf8');

const checks = [
  'startElapsedTimer()',
  'stopElapsedTimer()',
  "p.stage === 'fetching_version'",
  "p.stage === 'finding_asset'",
  "p.stage === 'downloading'",
  "p.stage === 'finalize'",
  "p.stage === 'complete'",
  '等待后端开始传输数据',
  '正在获取版本信息',
  '正在匹配安装包候选',
  '总耗时',
  'elapsed_timer'
];

console.log('=== Checking dist/main.js ===');
for (const ch of checks) {
  const count = c.split(ch).length - 1;
  console.log(`${ch}: ${count}`);
}

console.log('\n=== Verifying download listener ===');
const listenerStart = c.indexOf("downloadLlamaBtn?.addEventListener('click', async () => {");
const listenerEnd = c.indexOf('// ============ GPU', listenerStart);
const listener = c.substring(listenerStart, listenerEnd);

['fetching_version','finding_asset','downloading','extracting','finalize','complete','init']
  .forEach(s => {
    const cnt = listener.split("p.stage === '" + s + "'").length - 1;
    console.log(`${s} handler: ${cnt}`);
  });
console.log('startElapsedTimer in listener:', listener.split('startElapsedTimer').length - 1);
console.log('stopElapsedTimer in listener:', listener.split('stopElapsedTimer').length - 1);
