// ui/metricsView.mjs - Metrics display renderer
import { MESSAGES } from '../constants/messages.mjs';

/**
 * Metrics View - Renders CPU, VRAM, GPU usage
 * @param {Object} deps - Dependencies
 * @param {Object} deps.store - State store
 * @param {Object} deps.bus - Event bus
 * @param {Object} deps.elements - DOM elements cache
 * @param {number} deps.rafBudget - Max time to spend in RAF (ms)
 */
export function createMetricsView({ store, bus, elements, rafBudget = 16 }) {
  // Bind to metrics store
  const metricsKey = store.key('metrics');
  const unsubscribe = metricsKey.subscribe(renderMetrics);
  
  // Cache elements
  const els = elements || {
    metricCpuText: document.getElementById('metricCpuText'),
    metricCpuBar: document.getElementById('metricCpuBar'),
    metricVramText: document.getElementById('metricVramText'),
    metricVramBar: document.getElementById('metricVramBar'),
    metricGpuText: document.getElementById('metricGpuText'),
    metricGpuBar: document.getElementById('metricGpuBar')
  };
  
  // RAF batching
  let pendingMetrics = null;
  let rafScheduled = false;
  
  function renderMetrics(_, current) {
    if (!els.metricCpuText) return; // Not ready yet
    
    // Store metrics for RAF batching
    pendingMetrics = current;
    
    if (!rafScheduled) {
      rafScheduled = true;
      requestAnimationFrame(() => {
        rafScheduled = false;
        if (pendingMetrics) {
          applyMetrics(pendingMetrics);
          pendingMetrics = null;
        }
      });
    }
  }
  
  function applyMetrics(m) {
    // CPU
    if (m.cpuPct !== null && m.cpuPct !== undefined) {
      els.metricCpuText.textContent = `${m.cpuPct.toFixed(1)}%`;
      setMeterFill(els.metricCpuBar, m.cpuPct, { warn: 70, danger: 90 });
    } else {
      els.metricCpuText.textContent = MESSAGES.metricUnavailable;
      setMeterFill(els.metricCpuBar, 0, { unavailable: true });
    }
    
    // VRAM
    if (m.vramPct !== null && m.vramPct !== undefined) {
      const vramText = `${m.vramPct.toFixed(1)}% · ${m.vramUsed.toFixed(0)} / ${m.vramTotal.toFixed(0)} MB`;
      els.metricVramText.textContent = vramText;
      setMeterFill(els.metricVramBar, m.vramPct, { warn: 70, danger: 90 });
    } else {
      els.metricVramText.textContent = MESSAGES.metricUnavailable;
      setMeterFill(els.metricVramBar, 0, { unavailable: true });
    }
    
    // GPU
    if (m.gpuUtilPct !== null && m.gpuUtilPct !== undefined) {
      els.metricGpuText.textContent = `${m.gpuUtilPct.toFixed(1)}%`;
      setMeterFill(els.metricGpuBar, m.gpuUtilPct, { warn: 70, danger: 90 });
    } else {
      els.metricGpuText.textContent = MESSAGES.metricUnavailable;
      setMeterFill(els.metricGpuBar, 0, { unavailable: true });
    }
  }
  
  function setMeterFill(element, percent, options = {}) {
    if (!element) return;
    
    const { warn = 80, danger = 95, unavailable = false } = options;
    
    // Remove existing classes
    element.classList.remove('meter-fill-warning', 'meter-fill-danger', 'meter-fill-unavailable');
    
    // Set width
    element.style.width = `${Math.min(100, Math.max(0, percent))}%`;
    
    // Apply styling
    if (unavailable) {
      element.classList.add('meter-fill-unavailable');
    } else if (percent >= danger) {
      element.classList.add('meter-fill-danger');
    } else if (percent >= warn) {
      element.classList.add('meter-fill-warning');
    }
    // No class = normal (green)
  }
  
  // Listen for metrics updates from backend
  const metricsUnsubscribe = bus.on(EVENTS.SERVER_METRICS, (metrics) => {
    metricsKey.setState(metrics);
  });
  
  return {
    /** Clean up */
    destroy() {
      unsubscribe();
      metricsUnsubscribe.off();
    },
    
    /** Force refresh */
    refresh() {
      renderMetrics(undefined, metricsKey.get());
    }
  };
}

/** Default export */
export default createMetricsView;