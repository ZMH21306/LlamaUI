// app.mjs - LlamaUI 应用引导程序
// 将现有的模块化前端系统连接起来

import { createApp } from './app.bootstrap.mjs';

/**
 * LlamaUIApp - Application class
 * Instantiating this will bootstrap the app
 */
export class LlamaUIApp {
    constructor() {
        this._app = null;
        this._destroyed = false;
        
        // Bootstrap the app asynchronously
        createApp().then(app => {
            if (!this._destroyed) {
                this._app = app;
                // Expose on window for debugging
                window.__llamaApp = this;
            }
        }).catch(err => {
            console.error('Failed to initialize application:', err);
        });
    }
    
    /** Get the underlying store */
    get store() {
        return this._app?.store;
    }
    
    /** Get the event bus */
    get bus() {
        return this._app?.bus;
    }
    
    /** Get the service state machine */
    get serviceMachine() {
        return this._app?.serviceMachine;
    }
    
    /** Get all views */
    get views() {
        return this._app?.views;
    }
    
    /** Destroy the application */
    async destroy() {
        this._destroyed = true;
        if (this._app) {
            await this._app.destroy();
        }
    }
}