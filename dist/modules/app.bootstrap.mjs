// app.bootstrap.mjs - Minimal LlamaUI Bootstrap
// 实际的应用初始化逻辑（简化版）

import { EventBus, IPC } from './bus.mjs';
import { createStore } from './store.mjs';
import { createMachine } from './stateMachine.mjs';
import { EVENTS } from './constants/events.mjs';

import { createConfigView } from './ui/configView.mjs';
import { createLogView } from './ui/logView.mjs';
import { createMetricsView } from './ui/metricsView.mjs';
import { createServiceView } from './ui/serviceView.mjs';

/**
 * 创建并初始化 LlamaUI 应用
 * @returns {Promise<{store: Store, bus: EventBus, destroy: Function}>}
 */
export async function createApp() {
    // 基础设施
    const bus = new EventBus();
    const ipc = new IPC(window.__TAURI__);
    const store = createStore({
        service: { status: 'Stopped', pid: null, startedAt: null },
        config: { modelsDir: '', ctxSize: 2048, nGpuLayers: 0, port: 10897 },
        logs: { groups: new Map(), order: [] },
        metrics: { cpuPct: null, vramPct: null, vramUsed: null, vramTotal: null, gpuUtilPct: null },
        ui: { theme: 'dark' }
    });

    // 状态机
    const serviceMachine = createMachine({
        initialState: 'Stopped',
        on: {
            Stopped: {
                [EVENTS.UI_START_REQUESTED]: () => {
                    bus.emit(EVENTS.UI_CONFIG_DIRTY);
                    ipc.invoke(EVENTS.UI_START_REQUESTED)
                        .catch(err => console.error('[bootstrap] start failed:', err));
                    return 'Starting';
                },
            },
            Starting: {
                [EVENTS.SERVER_STATUS]: (status) => status === 'Running' ? 'Running' : 'Starting',
            },
            Running: {
                [EVENTS.UI_STOP_REQUESTED]: () => 'Stopping',
            },
            Stopping: {
                [EVENTS.SERVER_STATUS]: (status) => status === 'Stopped' ? 'Stopped' : 'Stopping',
            },
        },
    });

    // 缓存 DOM 元素
    const els = {
        statusText: document.querySelector('.status-text'),
        startBtn: document.getElementById('startBtn'),
        stopBtn: document.getElementById('stopBtn'),
        restartBtn: document.getElementById('restartBtn'),
        logs: document.getElementById('logs'),
    };

    // 初始化视图
    const views = {
        config: createConfigView({ store, bus, ipc, elements: els }),
        logs: createLogView({ store, bus, elements: els }),
        metrics: createMetricsView({ store, bus, elements: els, rafBudget: 16 }),
        service: createServiceView({ store, bus, ipc, elements: els }),
    };

    // 前端 → 前端事件流
    bus.on(EVENTS.UI_CONFIG_DIRTY, () => {
        store.setState({ configDirty: true }, 'configDirty');
    });

    // 主题切换
    bus.on(EVENTS.UI_THEME_CHANGE, (isLight) => {
        if (window.themeManager) {
            window.themeManager.setLightTheme(!!isLight);
        }
    });

    // 后端 → 前端事件流
    bus.on(EVENTS.SERVER_STATUS, (status) => {
        serviceMachine.transition(EVENTS.SERVER_STATUS, status);
        store.setState(status, 'service.status');
    });

    bus.on(EVENTS.SERVER_LOG, (log) => {
        // LogView 处理自己的日志
    });

    bus.on(EVENTS.SERVER_METRICS, (metrics) => {
        store.setState(metrics, 'metrics');
    });

    // 初始化
    async function init() {
        try {
            const status = await ipc.invoke(EVENTS.UI_INIT_REQUESTED);
            serviceMachine.transition(EVENTS.SERVER_STATUS, status);
            store.setState(status, 'service');
        } catch (err) {
            console.error('[bootstrap] init failed:', err);
        }

        ipc.listen(EVENTS.SERVER_STATUS, (status) => {
            serviceMachine.transition(EVENTS.SERVER_STATUS, status);
            store.setState(status, 'service.status');
        });
        ipc.listen(EVENTS.SERVER_METRICS, (metrics) => {
            store.setState(metrics, 'metrics');
        });
        ipc.listen(EVENTS.SERVER_LOG, (log) => {
            bus.emit(EVENTS.SERVER_LOG, log);
        });
    }

    await init();

    // 页面卸载保护
    window.addEventListener('beforeunload', async () => {
        const status = serviceMachine.state;
        if (['Starting', 'Running', 'Stopping'].includes(status)) {
            await ipc.invoke(EVENTS.UI_STOP_REQUESTED).catch(() => {});
        }
    });

    return {
        store,
        bus,
        serviceMachine,
        views,
        /** 销毁应用 */
        destroy: async () => {
            await ipc.invoke(EVENTS.UI_STOP_REQUESTED).catch(() => {});
            Object.values(views).forEach(view => view.destroy && view.destroy());
        },
    };
}