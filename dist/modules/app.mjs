// app.mjs - LlamaUI 应用引导程序
// 将现有的模块化前端系统连接起来

import { createApp } from './app.bootstrap.mjs';

// 初始化应用
createApp().catch(err => {
    console.error('Failed to initialize application:', err);
    // 可以在这里显示错误 UI
});