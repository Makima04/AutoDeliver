/**
 * AutoDeliver 管理后台 — 系统模块（定时任务 / 操作日志 / 通知设置 / 系统设置 / 管理员）
 *
 * 说明：
 * - 纯原生 JS，无构建步骤；依赖 index.html 的全局 showToast / openLoginModal，
 *   以及相同的管理员 token 约定（localStorage.adm_token）。
 * - 本文件在 index.html 内联脚本之前加载，顶层只做状态/常量初始化与函数定义，
 *   不立即调用 apiFetch / showToast。
 * - 每个页面暴露一个 window.pageLoaders[tab]，首次进入渲染完整骨架，之后只刷新数据。
 * - sysApi 镜像 apiFetch 的鉴权与 JSON 解析行为，同时保留 HTTP 状态码以识别 401。
 * - 所有动态 HTML 一律经过 sysEscape；接口字段严格遵循 docs/admin-modules-api.md。
 */

// ============================================================================
// 0. 全局状态与常量
// ============================================================================

var sysState = window.sysState || {
  scheduler: {
    ready: false,
    loading: false,
    tasks: [],
    runs: []
  },
  logs: {
    ready: false,
    loading: false,
    items: [],
    total: 0,
    limit: 50,
    offset: 0,
    stats: {},
    filters: { search: '', method: '', status: '' }
  },
  notifications: {
    ready: false,
    loading: false,
    channels: [],
    rules: [],
    logs: [],
    events: [],
    eventsError: false,
    editingConfig: {}
  },
  system: {
    ready: false,
    loading: false,
    info: null,
    settings: [],
    backups: []
  },
  admins: {
    ready: false,
    loading: false,
    me: null,
    authenticated: false,
    admins: [],
    logs: []
  }
};
window.sysState = sysState;

var SYS_AUTH_TEXT = '请先登录管理员账号';

var SYS_TASK_KINDS = [
  { value: 'auto_deliver_scan', label: '自动发货扫描' },
  { value: 'item_sync', label: '商品同步' },
  { value: 'message_sync', label: '消息同步' },
  { value: 'account_health', label: '账号健康检查' },
  { value: 'cleanup_logs', label: '日志清理' },
  { value: 'delivery_retry', label: '发货重试' },
  { value: 'notification_retry', label: '通知重试' }
];

var SYS_MIN_TASK_INTERVAL = 10;

var SYS_CHANNEL_TYPES = [
  { value: 'webhook', label: 'Webhook' },
  { value: 'feishu', label: '飞书' },
  { value: 'serverchan', label: 'ServerChan' },
  { value: 'email', label: '邮件' },
  { value: 'sms', label: '短信' }
];

var SYS_CHANNEL_CONFIG_FIELDS = {
  webhook: [
    { key: 'url', label: 'Webhook 地址', placeholder: 'https://example.com/hook', required: true },
    { key: 'secret', label: '签名密钥（可选）', placeholder: '用于请求签名', type: 'password' }
  ],
  feishu: [
    { key: 'webhook_url', label: '飞书机器人 Webhook', placeholder: 'https://open.feishu.cn/open-apis/bot/v2/hook/...', required: true }
  ],
  serverchan: [
    { key: 'send_key', label: 'ServerChan SendKey', placeholder: 'SCT...', type: 'password', required: true }
  ],
  email: [
    { key: 'gateway_url', label: '邮件网关地址', placeholder: 'https://...', required: true }
  ],
  sms: [
    { key: 'gateway_url', label: '短信网关地址', placeholder: 'https://...', required: true }
  ]
};

var SYS_NOTIFICATION_EVENTS_FALLBACK = [
  { event_type: 'delivery_success', label: '发货成功' },
  { event_type: 'delivery_failed', label: '发货失败' },
  { event_type: 'account_offline', label: '账号离线' },
  { event_type: 'out_of_stock', label: '库存不足' },
  { event_type: 'task_failed', label: '任务失败' },
  { event_type: 'order_refunded', label: '订单退款' },
  { event_type: 'system_alert', label: '系统告警' }
];

var SYS_EVENT_LABELS = {
  delivery_success: '发货成功',
  delivery_failed: '发货失败',
  account_offline: '账号离线',
  out_of_stock: '库存不足',
  task_failed: '任务失败',
  order_refunded: '订单退款',
  system_alert: '系统告警'
};

var SYS_SETTING_GROUP_LABELS = {
  basic: '基础信息',
  security: '安全',
  data: '数据与保留',
  delivery: '自动发货',
  message: '消息'
};

var SYS_ROLE_LABELS = {
  super_admin: '超级管理员',
  admin: '管理员',
  viewer: '只读'
};

var SYS_ADMIN_STATUS_LABELS = {
  active: '启用',
  disabled: '停用'
};

// ============================================================================
// 1. 通用工具
// ============================================================================

/** 所有动态内容进入 HTML 前必须转义。 */
function sysEscape(value) {
  if (value === null || value === undefined) return '';
  return String(value)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function sysAsArray(data) {
  if (Array.isArray(data)) return data;
  if (data && Array.isArray(data.items)) return data.items;
  return [];
}

function sysNum(value, fallback) {
  const n = Number(value);
  if (Number.isFinite(n)) return n;
  return fallback === undefined ? 0 : fallback;
}

function sysVal(id) {
  const el = document.getElementById(id);
  return el ? String(el.value == null ? '' : el.value).trim() : '';
}

/** 密码 / 密钥类输入不 trim，避免误伤首尾空格。 */
function sysRawVal(id) {
  const el = document.getElementById(id);
  return el ? String(el.value == null ? '' : el.value) : '';
}

function sysJsonOptions(method, body) {
  return {
    method: method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  };
}

function sysToast(msg, isError) {
  if (typeof showToast === 'function') {
    showToast(msg, !!isError);
    return;
  }
  if (isError) console.error(msg);
  else console.log(msg);
}

/** 时间统一去掉 SQLite 序列化里的 T，并截断到秒。 */
function sysFmtTime(value) {
  if (value === null || value === undefined || value === '') return '-';
  return String(value).replace('T', ' ').slice(0, 19);
}

function sysFmtMs(value) {
  if (value === null || value === undefined || value === '') return '-';
  return sysNum(value) + ' ms';
}

function sysFmtBytes(value) {
  const n = sysNum(value);
  if (n < 1024) return n + ' B';
  if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
  if (n < 1024 * 1024 * 1024) return (n / 1024 / 1024).toFixed(2) + ' MB';
  return (n / 1024 / 1024 / 1024).toFixed(2) + ' GB';
}

/** 运行时长（秒）格式化为天/小时/分/秒。 */
function sysFmtDuration(value) {
  const total = Math.max(0, Math.floor(sysNum(value)));
  const days = Math.floor(total / 86400);
  const hours = Math.floor((total % 86400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (days > 0) return days + ' 天 ' + hours + ' 小时 ' + minutes + ' 分';
  if (hours > 0) return hours + ' 小时 ' + minutes + ' 分';
  if (minutes > 0) return minutes + ' 分 ' + seconds + ' 秒';
  return seconds + ' 秒';
}

/**
 * 契约中金额单位为「分」，展示时统一转元。
 * 本模块 5 个页面当前没有分单位字段，保留该工具供后续复用。
 */
function sysFenToYuan(value) {
  return (sysNum(value) / 100).toFixed(2);
}

function sysTruncate(value, max) {
  const text = value === null || value === undefined ? '' : String(value);
  if (!text) return '';
  return text.length > max ? text.slice(0, max) + '…' : text;
}

/** 隐藏密钥/Key 中间部分，只保留前 4 位与后 4 位。 */
function sysMaskSecret(value) {
  if (value === null || value === undefined || value === '') return '未配置';
  const text = String(value);
  if (text.length <= 8) return new Array(text.length + 1).join('*');
  return text.slice(0, 4) + '****' + text.slice(-4);
}

function sysDateStamp() {
  const d = new Date();
  const pad = function (n) { return String(n).padStart(2, '0'); };
  return d.getFullYear() + pad(d.getMonth() + 1) + pad(d.getDate()) +
    '-' + pad(d.getHours()) + pad(d.getMinutes());
}

function sysSetText(id, text) {
  const el = document.getElementById(id);
  if (el) el.textContent = text === null || text === undefined ? '-' : String(text);
}

function sysBadge(text, tone) {
  const tones = {
    slate: 'bg-slate-100 text-slate-600 border-slate-200',
    indigo: 'bg-indigo-50 text-indigo-700 border-indigo-200',
    emerald: 'bg-emerald-50 text-emerald-700 border-emerald-200',
    rose: 'bg-rose-50 text-rose-700 border-rose-200',
    amber: 'bg-amber-50 text-amber-700 border-amber-200'
  };
  return '<span class="inline-flex items-center px-2 py-0.5 rounded-full border text-[11px] font-semibold ' +
    (tones[tone] || tones.slate) + '">' + sysEscape(text) + '</span>';
}

function sysStatCard(label, valueId, sub, valueClass) {
  return '<div class="bg-white rounded-xl border border-slate-200 shadow-sm p-5">' +
    '<div class="text-[11px] font-bold tracking-wide text-slate-500">' + sysEscape(label) + '</div>' +
    '<div id="' + valueId + '" class="mt-2 text-2xl font-bold ' + (valueClass || 'text-slate-900') + '">-</div>' +
    '<div class="mt-1 text-[11px] text-slate-400">' + sysEscape(sub || '') + '</div>' +
    '</div>';
}

function sysTableEmpty(colspan, title, hint) {
  return '<tr><td colspan="' + colspan + '" class="py-10 px-4 text-center">' +
    '<div class="text-sm font-semibold text-slate-500">' + sysEscape(title) + '</div>' +
    (hint ? '<div class="text-xs text-slate-400 mt-1">' + sysEscape(hint) + '</div>' : '') +
    '</td></tr>';
}

function sysEmptyBlock(title, hint, actionHtml) {
  return '<div class="text-center py-10 px-4">' +
    '<div class="text-sm font-semibold text-slate-500">' + sysEscape(title) + '</div>' +
    (hint ? '<div class="text-xs text-slate-400 mt-1">' + sysEscape(hint) + '</div>' : '') +
    (actionHtml ? '<div class="mt-3">' + actionHtml + '</div>' : '') +
    '</div>';
}

/**
 * 判定 401 是否属于「登录态失效」。
 * 后端对「需要超级管理员权限」「原密码不正确」「账号已被禁用」也返回 401，
 * 这类响应应 toast 原始错误，而不是清空 token 并弹登录提示。
 */
function sysIsAuthError(err) {
  if (!err || err.status !== 401) return false;
  const message = String(err.message || '');
  if (/权限|原密码|禁用/.test(message)) return false;
  return true;
}

function sysErrorBlockHtml(title, err) {
  return '<div class="text-center py-10 px-4">' +
    '<div class="text-sm font-semibold text-slate-500">' + sysEscape(title) + '</div>' +
    '<div class="text-xs text-slate-400 mt-1">' +
    sysEscape('加载失败：' + ((err && err.message) || '未知错误')) +
    '</div>' +
    '</div>';
}

function sysErrorBlock(elId, title, err, wrapClass) {
  const el = document.getElementById(elId);
  if (!el) return;
  const inner = sysErrorBlockHtml(title, err);
  el.innerHTML = wrapClass ? '<div class="' + wrapClass + '">' + inner + '</div>' : inner;
}

function sysErrorTable(tbodyId, title, err) {
  const tbody = document.getElementById(tbodyId);
  if (!tbody) return;
  const cols = tbody.getAttribute('data-colspan') || '6';
  tbody.innerHTML = sysTableEmpty(cols, title, '加载失败：' + ((err && err.message) || '未知错误'));
}

function sysAuthBannerHtml(page) {
  return '<div id="sys-auth-' + page + '" class="hidden rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 flex flex-wrap items-center justify-between gap-3">' +
    '<div class="flex items-center gap-2">' +
    '<span class="text-sm font-semibold text-amber-800">' + SYS_AUTH_TEXT + '</span>' +
    '<span class="text-xs text-amber-700">登录后可访问该页面数据。</span>' +
    '</div>' +
    '<div class="flex gap-2">' +
    '<button onclick="openLoginModal()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">登录</button>' +
    '<button onclick="sysReloadPage(\'' + page + '\')" class="px-3 py-1.5 rounded-lg bg-white border border-amber-300 text-amber-800 hover:bg-amber-100 text-xs font-semibold">刷新</button>' +
    '</div>' +
    '</div>';
}

function sysShowAuth(page) {
  const banner = document.getElementById('sys-auth-' + page);
  if (banner) banner.classList.remove('hidden');
  const view = document.getElementById('view-' + page);
  if (!view) return;
  view.querySelectorAll('[data-sys-loading-body]').forEach(function (tbody) {
    const cols = tbody.getAttribute('data-colspan') || '6';
    tbody.innerHTML = '<tr><td colspan="' + cols + '" class="py-10 px-4 text-center text-xs text-slate-400">' +
      SYS_AUTH_TEXT + '后查看</td></tr>';
  });
  view.querySelectorAll('[data-sys-loading-block]').forEach(function (el) {
    el.innerHTML = '<div class="py-8 text-center text-xs text-slate-400">' + SYS_AUTH_TEXT + '后查看</div>';
  });
}

function sysHideAuth(page) {
  const banner = document.getElementById('sys-auth-' + page);
  if (banner) banner.classList.add('hidden');
}

function sysReportResults(page, results) {
  let authShown = false;
  let toasted = false;
  (results || []).forEach(function (result) {
    if (!result || result.status !== 'rejected') return;
    const err = result.reason || {};
    if (sysIsAuthError(err)) {
      if (!authShown) {
        sysShowAuth(page);
        authShown = true;
      }
      return;
    }
    if (!toasted) {
      sysToast(err.message || '请求失败', true);
      toasted = true;
    }
  });
}

function sysHandleError(page, err) {
  if (!err) return;
  if (sysIsAuthError(err)) {
    sysShowAuth(page);
    return;
  }
  sysToast(err.message || '操作失败', true);
}

/**
 * 带登录态与管理端错误语义的 fetch（保留 HTTP 状态码，便于识别 401）。
 * 与 index.html 的 apiFetch 使用同一 token 约定：localStorage.adm_token。
 */
async function sysApi(url, options) {
  const opts = options || {};
  const token = localStorage.getItem('adm_token');
  const headers = new Headers(opts.headers || {});
  if (token) headers.set('Authorization', 'Bearer ' + token);
  const res = await fetch(url, Object.assign({}, opts, { headers: headers }));
  const raw = await res.text();
  let data = null;
  if (raw) {
    try {
      data = JSON.parse(raw);
    } catch (_) {
      const parseErr = new Error('接口 ' + url + ' 返回非 JSON 响应 (HTTP ' + res.status + ')');
      parseErr.status = res.status;
      throw parseErr;
    }
  } else {
    data = {};
  }
  if (!res.ok) {
    const err = new Error((data && data.message) || ('请求失败 (HTTP ' + res.status + ')'));
    err.status = res.status;
    err.data = data;
    if (sysIsAuthError(err)) localStorage.removeItem('adm_token');
    throw err;
  }
  return data;
}

// ============================================================================
// 2. 弹窗基础设施（动态挂到 body，避免修改 index.html）
// ============================================================================

function sysEnsureModal(id, innerHtml, maxWidth) {
  let overlay = document.getElementById(id);
  if (!overlay) {
    overlay = document.createElement('div');
    overlay.id = id;
    overlay.className = 'hidden fixed inset-0 bg-slate-900/50 z-50 flex items-center justify-center p-4';
    overlay.addEventListener('click', function (event) {
      if (event.target === overlay) overlay.classList.add('hidden');
    });
    document.body.appendChild(overlay);
  }
  overlay.innerHTML = '<div class="bg-white rounded-2xl border border-slate-200 shadow-xl ' +
    (maxWidth || 'max-w-lg') +
    ' w-full p-6 space-y-4 max-h-[90vh] overflow-y-auto" role="dialog" aria-modal="true">' +
    innerHtml + '</div>';
  return overlay;
}

function sysOpenModal(id) {
  const el = document.getElementById(id);
  if (el) el.classList.remove('hidden');
}

function sysCloseModal(id) {
  const el = document.getElementById(id);
  if (el) el.classList.add('hidden');
}

function sysModalCancelButton(modalId) {
  return '<button onclick="sysCloseModal(\'' + modalId + '\')" class="px-4 py-2 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">取消</button>';
}

function sysModalHeader(title, modalId) {
  return '<div class="flex items-center justify-between border-b border-slate-100 pb-3">' +
    '<h3 class="text-base font-bold text-slate-900">' + sysEscape(title) + '</h3>' +
    '<button onclick="sysCloseModal(\'' + modalId + '\')" class="text-slate-400 hover:text-slate-600">✕</button>' +
    '</div>';
}

// ============================================================================
// 3. 定时任务页（#view-scheduler）
// ============================================================================

function sysTaskStatusBadge(status) {
  const map = {
    idle: { label: '空闲', tone: 'slate' },
    running: { label: '运行中', tone: 'indigo' },
    success: { label: '成功', tone: 'emerald' },
    failed: { label: '失败', tone: 'rose' }
  };
  const item = map[status] || { label: status || '未知', tone: 'slate' };
  return sysBadge(item.label, item.tone);
}

function sysKindLabel(kind) {
  const found = SYS_TASK_KINDS.find(function (item) { return item.value === kind; });
  return found ? found.label : (kind || '-');
}

function sysRenderSchedulerSkeleton() {
  const view = document.getElementById('view-scheduler');
  if (!view) return;
  view.innerHTML =
    '<div id="sys-sched-root" class="space-y-4">' +
    sysAuthBannerHtml('scheduler') +
    '<div class="grid grid-cols-2 lg:grid-cols-4 gap-4">' +
    sysStatCard('任务总数', 'sys-sched-stat-total', '内置与自定义任务', 'text-slate-900') +
    sysStatCard('已启用', 'sys-sched-stat-enabled', '调度器每 5 秒检查', 'text-emerald-600') +
    sysStatCard('运行中', 'sys-sched-stat-running', 'last_status = running', 'text-indigo-600') +
    sysStatCard('最近失败', 'sys-sched-stat-failed', 'last_status = failed', 'text-rose-600') +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm">' +
    '<div class="px-6 py-4 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h2 class="text-sm font-bold text-slate-900">调度器概览</h2>' +
    '<p class="text-xs text-slate-500 mt-1">后台调度器每 5 秒检查一次到期任务；内置任务可修改执行间隔、启停或立即执行一次。</p>' +
    '</div>' +
    '<div class="flex gap-2">' +
    '<button onclick="sysSchedulerRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新</button>' +
    '<button onclick="sysOpenSchedulerNewModal()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">+ 新建任务</button>' +
    '</div>' +
    '</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">任务列表</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">间隔单位为秒，修改后点击「保存」；删除内置任务后调度器将不再执行它。</p>' +
    '</div>' +
    '<button onclick="sysSchedulerRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新任务</button>' +
    '</div>' +
    '<div class="overflow-x-auto">' +
    '<table class="w-full text-left">' +
    '<thead><tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">' +
    '<th class="py-3 px-4">任务</th>' +
    '<th class="py-3 px-4">类型 kind</th>' +
    '<th class="py-3 px-4">间隔</th>' +
    '<th class="py-3 px-4">状态</th>' +
    '<th class="py-3 px-4">上次运行 / 下次运行</th>' +
    '<th class="py-3 px-4">成功 / 失败</th>' +
    '<th class="py-3 px-4">耗时</th>' +
    '<th class="py-3 px-4">最后消息</th>' +
    '<th class="py-3 px-4">操作</th>' +
    '</tr></thead>' +
    '<tbody id="sys-sched-tasks-tbody" data-sys-loading-body data-colspan="9" class="divide-y divide-slate-100 text-sm">' +
    '<tr><td colspan="9" class="py-10 px-4 text-center text-slate-400 text-xs">正在加载定时任务…</td></tr>' +
    '</tbody>' +
    '</table>' +
    '</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">执行历史</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">最近 30 次任务执行结果（GET /api/scheduler/runs?limit=30）。</p>' +
    '</div>' +
    '<button onclick="sysSchedulerRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新历史</button>' +
    '</div>' +
    '<div class="overflow-x-auto">' +
    '<table class="w-full text-left">' +
    '<thead><tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">' +
    '<th class="py-3 px-4">task_key</th>' +
    '<th class="py-3 px-4">状态</th>' +
    '<th class="py-3 px-4">消息</th>' +
    '<th class="py-3 px-4">耗时</th>' +
    '<th class="py-3 px-4">开始时间</th>' +
    '</tr></thead>' +
    '<tbody id="sys-sched-runs-tbody" data-sys-loading-body data-colspan="5" class="divide-y divide-slate-100 text-sm">' +
    '<tr><td colspan="5" class="py-10 px-4 text-center text-slate-400 text-xs">正在加载执行历史…</td></tr>' +
    '</tbody>' +
    '</table>' +
    '</div>' +
    '</div>' +
    '</div>';
}

function sysRenderSchedulerStats() {
  const tasks = sysState.scheduler.tasks || [];
  sysSetText('sys-sched-stat-total', tasks.length);
  sysSetText('sys-sched-stat-enabled', tasks.filter(function (t) { return !!t.enabled; }).length);
  sysSetText('sys-sched-stat-running', tasks.filter(function (t) { return t.last_status === 'running'; }).length);
  sysSetText('sys-sched-stat-failed', tasks.filter(function (t) { return t.last_status === 'failed'; }).length);
}

function sysRenderSchedulerTasks() {
  const tbody = document.getElementById('sys-sched-tasks-tbody');
  if (!tbody) return;
  const tasks = sysState.scheduler.tasks || [];
  if (!tasks.length) {
    tbody.innerHTML = sysTableEmpty(9, '暂无定时任务', '点击「+ 新建任务」注册第一个任务，或确认后端已补齐内置任务。');
    return;
  }
  tbody.innerHTML = tasks.map(function (task) {
    const id = sysNum(task.id);
    const enabled = !!task.enabled;
    const failCount = sysNum(task.fail_count);
    return '<tr class="hover:bg-slate-50/80 align-top">' +
      '<td class="py-3 px-4">' +
      '<div class="text-xs font-bold text-slate-900">' + sysEscape(task.name || '-') + '</div>' +
      '<div class="text-[11px] font-mono text-slate-400 mt-0.5">' + sysEscape(task.task_key || '-') + '</div>' +
      '</td>' +
      '<td class="py-3 px-4">' +
      '<div class="text-xs text-slate-700">' + sysEscape(sysKindLabel(task.kind)) + '</div>' +
      '<div class="text-[11px] font-mono text-slate-400 mt-0.5">' + sysEscape(task.kind || '-') + '</div>' +
      '</td>' +
      '<td class="py-3 px-4">' +
      '<div class="flex items-center gap-1.5">' +
      '<input id="sys-task-interval-' + id + '" type="number" min="10" value="' + sysEscape(task.interval_seconds) + '" class="w-20 rounded-lg border border-slate-300 px-2 py-1 text-xs font-mono focus:outline-none focus:border-indigo-500" />' +
      '<span class="text-[11px] text-slate-400">秒</span>' +
      '<button onclick="sysSchedulerSaveInterval(' + id + ')" class="px-2 py-1 rounded-lg bg-indigo-50 hover:bg-indigo-100 text-indigo-700 text-[11px] font-semibold">保存</button>' +
      '</div>' +
      '</td>' +
      '<td class="py-3 px-4">' +
      '<div class="flex flex-col gap-1 items-start">' +
      sysTaskStatusBadge(task.last_status) +
      sysBadge(enabled ? '已启用' : '已停用', enabled ? 'emerald' : 'slate') +
      '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-[11px] text-slate-500 font-mono whitespace-nowrap">' +
      '<div>上次: ' + sysEscape(sysFmtTime(task.last_run_at)) + '</div>' +
      '<div>下次: ' + sysEscape(sysFmtTime(task.next_run_at)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-xs whitespace-nowrap">' +
      '<span class="font-semibold text-emerald-600">' + sysNum(task.run_count) + '</span>' +
      '<span class="text-slate-300"> / </span>' +
      '<span class="font-semibold ' + (failCount > 0 ? 'text-rose-600' : 'text-slate-400') + '">' + failCount + '</span>' +
      '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-500 font-mono whitespace-nowrap">' + sysEscape(sysFmtMs(task.last_duration_ms)) + '</td>' +
      '<td class="py-3 px-4">' +
      '<div class="text-xs text-slate-500 max-w-[220px] truncate" title="' + sysEscape(task.last_message || '') + '">' + sysEscape(sysTruncate(task.last_message || '-', 40)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4">' +
      '<div class="flex flex-wrap gap-1.5">' +
      '<button onclick="sysSchedulerToggle(' + id + ', ' + (enabled ? 'false' : 'true') + ')" class="px-2.5 py-1 rounded-lg ' + (enabled ? 'bg-slate-100 hover:bg-slate-200 text-slate-700' : 'bg-emerald-50 hover:bg-emerald-100 text-emerald-700') + ' text-[11px] font-medium">' + (enabled ? '停用' : '启用') + '</button>' +
      '<button id="sys-task-run-' + id + '" onclick="sysSchedulerRun(' + id + ')" class="px-2.5 py-1 rounded-lg bg-indigo-50 hover:bg-indigo-100 text-indigo-700 text-[11px] font-medium">立即执行</button>' +
      '<button onclick="sysSchedulerDelete(' + id + ')" class="px-2.5 py-1 rounded-lg bg-rose-50 hover:bg-rose-100 text-rose-600 text-[11px] font-medium">删除</button>' +
      '</div>' +
      '</td>' +
      '</tr>';
  }).join('');
}

function sysRenderSchedulerRuns() {
  const tbody = document.getElementById('sys-sched-runs-tbody');
  if (!tbody) return;
  const runs = sysState.scheduler.runs || [];
  if (!runs.length) {
    tbody.innerHTML = sysTableEmpty(5, '暂无执行历史', '任务执行一次后，这里会展示 task_key、状态、消息与耗时。');
    return;
  }
  tbody.innerHTML = runs.map(function (run) {
    return '<tr class="hover:bg-slate-50/80">' +
      '<td class="py-3 px-4 text-xs font-mono text-slate-700">' + sysEscape(run.task_key || '-') + '</td>' +
      '<td class="py-3 px-4">' + sysTaskStatusBadge(run.status) + '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-500 max-w-xs">' +
      '<div class="truncate" title="' + sysEscape(run.message || '') + '">' + sysEscape(sysTruncate(run.message || '-', 80)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-xs font-mono text-slate-500 whitespace-nowrap">' + sysEscape(sysFmtMs(run.duration_ms)) + '</td>' +
      '<td class="py-3 px-4 text-[11px] font-mono text-slate-400 whitespace-nowrap">' + sysEscape(sysFmtTime(run.started_at)) + '</td>' +
      '</tr>';
  }).join('');
}

async function sysSchedulerFetchTasks() {
  const list = await sysApi('/api/scheduler/tasks');
  sysState.scheduler.tasks = sysAsArray(list);
  sysRenderSchedulerStats();
  sysRenderSchedulerTasks();
  sysSetText('badge-scheduler', sysState.scheduler.tasks.length);
}

async function sysSchedulerFetchRuns() {
  const list = await sysApi('/api/scheduler/runs?limit=30');
  sysState.scheduler.runs = sysAsArray(list);
  sysRenderSchedulerRuns();
}

async function sysSchedulerRefreshAll() {
  const st = sysState.scheduler;
  if (st.loading) return;
  st.loading = true;
  try {
    const results = await Promise.allSettled([
      sysSchedulerFetchTasks(),
      sysSchedulerFetchRuns()
    ]);
    sysReportResults('scheduler', results);
    if (results[0].status === 'rejected' && !sysIsAuthError(results[0].reason)) {
      sysErrorTable('sys-sched-tasks-tbody', '定时任务加载失败', results[0].reason);
    }
    if (results[1].status === 'rejected' && !sysIsAuthError(results[1].reason)) {
      sysErrorTable('sys-sched-runs-tbody', '执行历史加载失败', results[1].reason);
    }
    if (results.some(function (r) { return r.status === 'fulfilled'; })) {
      sysHideAuth('scheduler');
    }
  } catch (e) {
    sysHandleError('scheduler', e);
  } finally {
    st.loading = false;
  }
}

async function loadSysScheduler() {
  try {
    const st = sysState.scheduler;
    if (!st.ready || !document.getElementById('sys-sched-root')) {
      sysRenderSchedulerSkeleton();
      st.ready = true;
    }
    await sysSchedulerRefreshAll();
  } catch (e) {
    sysHandleError('scheduler', e);
  }
}

async function sysSchedulerSaveInterval(id) {
  const input = document.getElementById('sys-task-interval-' + id);
  const seconds = input ? parseInt(input.value, 10) : NaN;
  if (!Number.isFinite(seconds) || seconds < SYS_MIN_TASK_INTERVAL) {
    return sysToast('执行间隔不能小于 ' + SYS_MIN_TASK_INTERVAL + ' 秒', true);
  }
  try {
    await sysApi('/api/scheduler/tasks/' + id, sysJsonOptions('PUT', { interval_seconds: seconds }));
    sysToast('执行间隔已更新');
    await sysSchedulerRefreshAll();
  } catch (e) {
    sysHandleError('scheduler', e);
  }
}

async function sysSchedulerToggle(id, enabled) {
  try {
    await sysApi('/api/scheduler/tasks/' + id, sysJsonOptions('PUT', { enabled: !!enabled }));
    sysToast(enabled ? '任务已启用' : '任务已停用');
    await sysSchedulerRefreshAll();
  } catch (e) {
    sysHandleError('scheduler', e);
  }
}

async function sysSchedulerRun(id) {
  const btn = document.getElementById('sys-task-run-' + id);
  if (btn) {
    btn.disabled = true;
    btn.textContent = '执行中…';
    btn.classList.add('opacity-60');
  }
  try {
    const res = await sysApi('/api/scheduler/tasks/' + id + '/run', { method: 'POST' });
    sysToast((res && res.message) || '任务已执行');
    await sysSchedulerRefreshAll();
  } catch (e) {
    sysHandleError('scheduler', e);
  } finally {
    if (btn && document.body.contains(btn)) {
      btn.disabled = false;
      btn.textContent = '立即执行';
      btn.classList.remove('opacity-60');
    }
  }
}

async function sysSchedulerDelete(id) {
  const task = (sysState.scheduler.tasks || []).find(function (t) { return sysNum(t.id) === sysNum(id); });
  const name = task ? task.name : String(id);
  const extra = task && task.task_key ? '（task_key: ' + task.task_key + '）' : '';
  if (!window.confirm('确认删除定时任务「' + name + '」' + extra + '？删除后调度器将不再执行该任务。')) return;
  try {
    await sysApi('/api/scheduler/tasks/' + id, { method: 'DELETE' });
    sysToast('定时任务已删除');
    await sysSchedulerRefreshAll();
  } catch (e) {
    sysHandleError('scheduler', e);
  }
}

function sysOpenSchedulerNewModal() {
  const kindOptions = SYS_TASK_KINDS.map(function (kind) {
    return '<option value="' + sysEscape(kind.value) + '">' + sysEscape(kind.label) + ' (' + sysEscape(kind.value) + ')</option>';
  }).join('');
  sysEnsureModal('sys-scheduler-modal',
    sysModalHeader('新建定时任务', 'sys-scheduler-modal') +
    '<div class="space-y-3 text-xs">' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">task_key *</label>' +
    '<input id="sys-task-key" type="text" placeholder="例如：my_custom_task" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono" />' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">任务名称 *</label>' +
    '<input id="sys-task-name" type="text" placeholder="例如：自定义巡检任务" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm" />' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">任务类型 kind</label>' +
    '<select id="sys-task-kind" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm">' + kindOptions + '</select>' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">执行间隔（秒）*</label>' +
    '<input id="sys-task-interval" type="number" min="10" value="300" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono" />' +
    '</div>' +
    '<label class="flex items-center gap-2 rounded-lg border border-slate-200 bg-slate-50 px-3 py-2.5 cursor-pointer">' +
    '<input id="sys-task-enabled" type="checkbox" checked class="w-4 h-4 rounded border-slate-300 text-indigo-600" />' +
    '<span class="text-xs font-medium text-slate-700">创建后立即启用</span>' +
    '</label>' +
    '<div class="text-[11px] text-slate-400">task_key 需全局唯一；kind 决定后端执行逻辑，可选 7 种内置类型。</div>' +
    '</div>' +
    '<div class="flex justify-end gap-2 pt-2">' +
    sysModalCancelButton('sys-scheduler-modal') +
    '<button onclick="sysSchedulerCreateTask()" class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">创建任务</button>' +
    '</div>'
  );
  sysOpenModal('sys-scheduler-modal');
}

async function sysSchedulerCreateTask() {
  const taskKey = sysVal('sys-task-key');
  const name = sysVal('sys-task-name');
  const kind = sysVal('sys-task-kind') || SYS_TASK_KINDS[0].value;
  const interval = parseInt(sysVal('sys-task-interval'), 10);
  const enabledEl = document.getElementById('sys-task-enabled');
  const enabled = enabledEl ? enabledEl.checked : true;
  if (!taskKey) return sysToast('请填写 task_key', true);
  if (!name) return sysToast('请填写任务名称', true);
  if (!Number.isFinite(interval) || interval < SYS_MIN_TASK_INTERVAL) return sysToast('执行间隔不能小于 ' + SYS_MIN_TASK_INTERVAL + ' 秒', true);
  try {
    await sysApi('/api/scheduler/tasks', sysJsonOptions('POST', {
      task_key: taskKey,
      name: name,
      kind: kind,
      interval_seconds: interval,
      enabled: enabled
    }));
    sysCloseModal('sys-scheduler-modal');
    sysToast('定时任务已创建');
    await sysSchedulerRefreshAll();
  } catch (e) {
    if (sysIsAuthError(e)) sysCloseModal('sys-scheduler-modal');
    sysHandleError('scheduler', e);
  }
}

// ============================================================================
// 4. 操作日志页（#view-logs）
// ============================================================================

function sysHttpMethodBadge(method) {
  const map = { GET: 'slate', POST: 'indigo', PUT: 'amber', PATCH: 'amber', DELETE: 'rose' };
  return sysBadge(method || '-', map[method] || 'slate');
}

function sysStatusCodeBadge(code) {
  const n = sysNum(code);
  const tone = n >= 500 ? 'rose' : (n >= 400 ? 'amber' : (n >= 200 ? 'emerald' : 'slate'));
  return sysBadge('HTTP ' + n, tone);
}

function sysRenderLogsSkeleton() {
  const view = document.getElementById('view-logs');
  if (!view) return;
  view.innerHTML =
    '<div id="sys-logs-root" class="space-y-4">' +
    sysAuthBannerHtml('logs') +
    '<div class="grid grid-cols-2 lg:grid-cols-4 gap-4">' +
    sysStatCard('日志总数', 'sys-logs-stat-total', '全部操作审计', 'text-slate-900') +
    sysStatCard('今日日志', 'sys-logs-stat-today', '今天新增的操作记录', 'text-indigo-600') +
    sysStatCard('失败请求', 'sys-logs-stat-failed', 'HTTP 状态码 ≥ 400', 'text-rose-600') +
    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm p-5">' +
    '<div class="text-[11px] font-bold tracking-wide text-slate-500">热门路径 TOP</div>' +
    '<div id="sys-logs-top-paths" class="mt-2 space-y-1.5"><div class="text-xs text-slate-400">加载中…</div></div>' +
    '</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm p-4 space-y-3">' +
    '<div class="flex flex-wrap items-center gap-2">' +
    '<input id="sys-logs-search" type="search" placeholder="搜索操作人 / action / 路径 / 详情" onkeydown="sysLogsSearchKeydown(event)" class="flex-1 min-w-[220px] rounded-lg border border-slate-300 px-3 py-2 text-sm focus:outline-none focus:border-indigo-500" />' +
    '<select id="sys-logs-method" onchange="sysLogsApplyFilters()" class="rounded-lg border border-slate-300 px-3 py-2 text-xs bg-white">' +
    '<option value="">全部方法</option>' +
    '<option value="GET">GET</option>' +
    '<option value="POST">POST</option>' +
    '<option value="PUT">PUT</option>' +
    '<option value="PATCH">PATCH</option>' +
    '<option value="DELETE">DELETE</option>' +
    '</select>' +
    '<select id="sys-logs-status" onchange="sysLogsApplyFilters()" class="rounded-lg border border-slate-300 px-3 py-2 text-xs bg-white">' +
    '<option value="">全部结果</option>' +
    '<option value="success">成功</option>' +
    '<option value="failed">失败</option>' +
    '</select>' +
    '<button onclick="sysLogsApplyFilters()" class="px-3 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">查询</button>' +
    '<button onclick="sysLogsRefreshAll()" class="px-3 py-2 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新</button>' +
    '<button onclick="sysLogsExport()" class="px-3 py-2 rounded-lg bg-emerald-50 hover:bg-emerald-100 text-emerald-700 text-xs font-semibold">导出 CSV</button>' +
    '</div>' +
    '<div class="flex flex-wrap items-center gap-2 border-t border-slate-100 pt-3">' +
    '<span class="text-[11px] font-semibold text-rose-600">清理日志</span>' +
    '<span class="text-[11px] text-slate-400">删除</span>' +
    '<input id="sys-logs-cleanup-days" type="number" min="1" value="30" class="w-20 rounded-lg border border-slate-300 px-2 py-1.5 text-xs font-mono" />' +
    '<span class="text-[11px] text-slate-400">天前的操作日志</span>' +
    '<button onclick="sysLogsCleanup()" class="px-3 py-1.5 rounded-lg bg-rose-50 hover:bg-rose-100 text-rose-600 text-xs font-medium">执行清理</button>' +
    '</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">日志列表</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">由审计中间件自动记录管理端 POST / PUT / PATCH / DELETE 操作，详情可点击查看。</p>' +
    '</div>' +
    '<button onclick="sysLogsRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新列表</button>' +
    '</div>' +
    '<div class="overflow-x-auto">' +
    '<table class="w-full text-left">' +
    '<thead><tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">' +
    '<th class="py-3 px-4">时间</th>' +
    '<th class="py-3 px-4">操作人</th>' +
    '<th class="py-3 px-4">操作 action</th>' +
    '<th class="py-3 px-4">方法</th>' +
    '<th class="py-3 px-4">路径</th>' +
    '<th class="py-3 px-4">状态码</th>' +
    '<th class="py-3 px-4">耗时</th>' +
    '<th class="py-3 px-4">IP</th>' +
    '<th class="py-3 px-4">详情</th>' +
    '</tr></thead>' +
    '<tbody id="sys-logs-tbody" data-sys-loading-body data-colspan="9" class="divide-y divide-slate-100 text-sm">' +
    '<tr><td colspan="9" class="py-10 px-4 text-center text-slate-400 text-xs">正在加载操作日志…</td></tr>' +
    '</tbody>' +
    '</table>' +
    '</div>' +
    '<div class="px-6 py-3 border-t border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div id="sys-logs-page-info" class="text-[11px] text-slate-500">-</div>' +
    '<div class="flex gap-2">' +
    '<button id="sys-logs-prev" onclick="sysLogsPage(-1)" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">上一页</button>' +
    '<button id="sys-logs-next" onclick="sysLogsPage(1)" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">下一页</button>' +
    '</div>' +
    '</div>' +
    '</div>' +
    '</div>';
}

function sysLogsFiltersFromDom() {
  const searchEl = document.getElementById('sys-logs-search');
  const methodEl = document.getElementById('sys-logs-method');
  const statusEl = document.getElementById('sys-logs-status');
  const fallback = sysState.logs.filters || { search: '', method: '', status: '' };
  return {
    search: searchEl ? searchEl.value.trim() : fallback.search,
    method: methodEl ? methodEl.value : fallback.method,
    status: statusEl ? statusEl.value : fallback.status
  };
}

function sysRenderLogsStats() {
  const stats = sysState.logs.stats || {};
  sysSetText('sys-logs-stat-total', sysNum(stats.total));
  sysSetText('sys-logs-stat-today', sysNum(stats.today));
  sysSetText('sys-logs-stat-failed', sysNum(stats.failed));
  const topEl = document.getElementById('sys-logs-top-paths');
  if (topEl) {
    const top = Array.isArray(stats.top_paths) ? stats.top_paths.slice(0, 4) : [];
    topEl.innerHTML = top.length ? top.map(function (item) {
      return '<div class="flex items-center justify-between gap-3">' +
        '<span class="font-mono text-[11px] text-slate-600 truncate" title="' + sysEscape(item.path || '') + '">' + sysEscape(item.path || '-') + '</span>' +
        '<span class="text-xs font-bold text-slate-900 whitespace-nowrap">' + sysEscape(item.count) + '</span>' +
        '</div>';
    }).join('') : '<div class="text-xs text-slate-400">暂无数据</div>';
  }
}

function sysRenderLogsTable() {
  const tbody = document.getElementById('sys-logs-tbody');
  if (!tbody) return;
  const items = sysState.logs.items || [];
  if (!items.length) {
    tbody.innerHTML = sysTableEmpty(9, '暂无操作日志', '调整筛选条件，或执行一次后台变更操作后再刷新。');
    sysRenderLogsPager();
    return;
  }
  tbody.innerHTML = items.map(function (log) {
    const id = sysNum(log.id);
    return '<tr class="hover:bg-slate-50/80 align-top">' +
      '<td class="py-3 px-4 text-[11px] font-mono text-slate-400 whitespace-nowrap">' + sysEscape(sysFmtTime(log.created_at)) + '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-700">' + sysEscape(log.actor || '-') + '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-700 max-w-[200px]">' +
      '<div class="truncate" title="' + sysEscape(log.action || '') + '">' + sysEscape(sysTruncate(log.action || '-', 36)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4">' + sysHttpMethodBadge(log.method) + '</td>' +
      '<td class="py-3 px-4 text-[11px] font-mono text-slate-600 max-w-[220px]">' +
      '<div class="truncate" title="' + sysEscape(log.path || '') + '">' + sysEscape(log.path || '-') + '</div>' +
      '</td>' +
      '<td class="py-3 px-4">' + sysStatusCodeBadge(log.status_code) + '</td>' +
      '<td class="py-3 px-4 text-xs font-mono text-slate-500 whitespace-nowrap">' + sysEscape(sysFmtMs(log.duration_ms)) + '</td>' +
      '<td class="py-3 px-4 text-[11px] font-mono text-slate-500 whitespace-nowrap">' + sysEscape(log.ip || '-') + '</td>' +
      '<td class="py-3 px-4">' +
      '<div class="flex items-center gap-2">' +
      '<span class="text-xs text-slate-500 max-w-[180px] truncate" title="' + sysEscape(log.detail || '') + '">' + sysEscape(sysTruncate(log.detail || '-', 28)) + '</span>' +
      '<button onclick="sysLogsShowDetail(' + id + ')" class="px-2 py-0.5 rounded bg-indigo-50 hover:bg-indigo-100 text-indigo-700 text-[11px] font-medium whitespace-nowrap">查看</button>' +
      '<button onclick="sysLogsDelete(' + id + ')" class="px-2 py-0.5 rounded bg-rose-50 hover:bg-rose-100 text-rose-600 text-[11px] font-medium">删除</button>' +
      '</div>' +
      '</td>' +
      '</tr>';
  }).join('');
  sysRenderLogsPager();
}

function sysRenderLogsPager() {
  const st = sysState.logs;
  const from = st.total === 0 ? 0 : st.offset + 1;
  const to = Math.min(st.offset + st.limit, st.total);
  const info = document.getElementById('sys-logs-page-info');
  if (info) info.textContent = '第 ' + from + '-' + to + ' 条 / 共 ' + st.total + ' 条';
  const baseCls = 'px-3 py-1.5 rounded-lg bg-slate-100 text-slate-700 text-xs font-medium';
  const disabledCls = ' opacity-40 cursor-not-allowed';
  const prev = document.getElementById('sys-logs-prev');
  if (prev) {
    prev.disabled = st.offset <= 0;
    prev.className = 'px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium' + (st.offset <= 0 ? disabledCls : '');
  }
  const next = document.getElementById('sys-logs-next');
  if (next) {
    const atEnd = st.offset + st.limit >= st.total;
    next.disabled = atEnd;
    next.className = baseCls + ' hover:bg-slate-200' + (atEnd ? disabledCls : '');
  }
}

async function sysLogsFetchStats() {
  const stats = await sysApi('/api/logs/stats');
  sysState.logs.stats = stats || {};
  sysRenderLogsStats();
  if (sysState.logs.stats && sysState.logs.stats.total !== undefined) {
    sysSetText('badge-logs', sysNum(sysState.logs.stats.total));
  }
}

var sysLogsListSeq = 0;

async function sysLogsFetchList() {
  const st = sysState.logs;
  const seq = ++sysLogsListSeq;
  const filters = sysLogsFiltersFromDom();
  st.filters = filters;
  const params = new URLSearchParams();
  if (filters.search) params.set('search', filters.search);
  if (filters.method) params.set('method', filters.method);
  if (filters.status) params.set('status', filters.status);
  params.set('limit', String(st.limit));
  params.set('offset', String(st.offset));
  const data = await sysApi('/api/logs?' + params.toString());
  // 快速切换筛选时丢弃过期响应，避免旧结果覆盖新筛选。
  if (seq !== sysLogsListSeq) return;
  st.items = Array.isArray(data) ? data : sysAsArray(data.items);
  st.total = sysNum(data && data.total, st.items.length);
  if (data && data.limit) st.limit = sysNum(data.limit, st.limit);
  if (data && data.offset !== undefined && data.offset !== null) st.offset = sysNum(data.offset, st.offset);
  sysRenderLogsTable();
}

async function sysLogsRefreshAll() {
  const st = sysState.logs;
  if (st.loading) return;
  st.loading = true;
  try {
    const results = await Promise.allSettled([
      sysLogsFetchStats(),
      sysLogsFetchList()
    ]);
    sysReportResults('logs', results);
    if (results[0].status === 'rejected' && !sysIsAuthError(results[0].reason)) {
      sysErrorBlock('sys-logs-top-paths', '统计加载失败', results[0].reason);
    }
    if (results[1].status === 'rejected' && !sysIsAuthError(results[1].reason)) {
      sysErrorTable('sys-logs-tbody', '操作日志加载失败', results[1].reason);
    }
    if (results.some(function (r) { return r.status === 'fulfilled'; })) {
      sysHideAuth('logs');
    }
  } catch (e) {
    sysHandleError('logs', e);
  } finally {
    st.loading = false;
  }
}

async function loadSysLogs() {
  try {
    const st = sysState.logs;
    if (!st.ready || !document.getElementById('sys-logs-root')) {
      sysRenderLogsSkeleton();
      st.ready = true;
    }
    await sysLogsRefreshAll();
  } catch (e) {
    sysHandleError('logs', e);
  }
}

function sysLogsSearchKeydown(event) {
  if (event && event.key === 'Enter') sysLogsApplyFilters();
}

function sysLogsApplyFilters() {
  sysState.logs.offset = 0;
  sysLogsFetchList()
    .then(function () { sysHideAuth('logs'); })
    .catch(function (e) { sysHandleError('logs', e); });
}

function sysLogsPage(direction) {
  const st = sysState.logs;
  const nextOffset = st.offset + direction * st.limit;
  if (nextOffset < 0) return;
  if (nextOffset >= st.total && direction > 0) return;
  st.offset = nextOffset;
  sysLogsFetchList()
    .then(function () { sysHideAuth('logs'); })
    .catch(function (e) { sysHandleError('logs', e); });
}

function sysLogsShowDetail(id) {
  const log = (sysState.logs.items || []).find(function (item) { return sysNum(item.id) === sysNum(id); });
  if (!log) return;
  const rows = [
    ['时间', sysFmtTime(log.created_at)],
    ['操作人', log.actor || '-'],
    ['操作 action', log.action || '-'],
    ['方法', log.method || '-'],
    ['路径', log.path || '-'],
    ['状态码', log.status_code],
    ['耗时', sysFmtMs(log.duration_ms)],
    ['IP', log.ip || '-'],
    ['资源类型', log.resource_type || '-'],
    ['资源 ID', log.resource_id || '-'],
    ['User-Agent', log.user_agent || '-']
  ];
  const rowsHtml = rows.map(function (row) {
    return '<div class="flex gap-3 py-1.5 border-b border-slate-100 last:border-0">' +
      '<div class="w-28 shrink-0 text-[11px] font-semibold text-slate-500">' + sysEscape(row[0]) + '</div>' +
      '<div class="text-xs text-slate-800 break-all">' + sysEscape(row[1]) + '</div>' +
      '</div>';
  }).join('');
  sysEnsureModal('sys-log-detail-modal',
    sysModalHeader('操作日志详情 #' + sysNum(log.id), 'sys-log-detail-modal') +
    '<div>' + rowsHtml + '</div>' +
    '<div>' +
    '<div class="text-[11px] font-semibold text-slate-500 mb-1">详情 detail</div>' +
    '<pre class="rounded-xl border border-slate-200 bg-slate-50 p-3 text-[11px] text-slate-700 whitespace-pre-wrap break-all max-h-60 overflow-y-auto">' +
    sysEscape(log.detail || '无详情') + '</pre>' +
    '</div>' +
    '<div class="flex justify-end pt-1">' +
    '<button onclick="sysCloseModal(\'sys-log-detail-modal\')" class="px-4 py-2 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">关闭</button>' +
    '</div>',
    'max-w-2xl'
  );
  sysOpenModal('sys-log-detail-modal');
}

async function sysLogsDelete(id) {
  if (!window.confirm('确认删除这条操作日志？该操作不可恢复。')) return;
  try {
    await sysApi('/api/logs/' + id, { method: 'DELETE' });
    sysToast('操作日志已删除');
    if (sysState.logs.items.length === 1 && sysState.logs.offset > 0) {
      sysState.logs.offset = Math.max(0, sysState.logs.offset - sysState.logs.limit);
    }
    await sysLogsRefreshAll();
  } catch (e) {
    sysHandleError('logs', e);
  }
}

async function sysLogsCleanup() {
  const days = parseInt(sysVal('sys-logs-cleanup-days'), 10);
  if (!Number.isFinite(days) || days < 1) return sysToast('请输入大于 0 的保留天数', true);
  if (!window.confirm('确认清理 ' + days + ' 天前的操作日志？该操作不可恢复。')) return;
  try {
    const res = await sysApi('/api/logs?before_days=' + days, { method: 'DELETE' });
    sysToast('已清理 ' + sysNum(res && res.deleted) + ' 条操作日志');
    sysState.logs.offset = 0;
    await sysLogsRefreshAll();
  } catch (e) {
    sysHandleError('logs', e);
  }
}

async function sysLogsExport() {
  try {
    const token = localStorage.getItem('adm_token');
    const headers = {};
    if (token) headers.Authorization = 'Bearer ' + token;
    const res = await fetch('/api/logs/export', { headers: headers });
    if (!res.ok) {
      let message = '导出失败 (HTTP ' + res.status + ')';
      try {
        const data = await res.json();
        if (data && data.message) message = data.message;
      } catch (_) { /* 非 JSON 错误体 */ }
      const err = new Error(message);
      err.status = res.status;
      if (sysIsAuthError(err)) {
        localStorage.removeItem('adm_token');
        sysShowAuth('logs');
        throw new Error(SYS_AUTH_TEXT);
      }
      throw err;
    }
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = 'operation-logs-' + sysDateStamp() + '.csv';
    document.body.appendChild(link);
    link.click();
    link.remove();
    window.setTimeout(function () { URL.revokeObjectURL(url); }, 1000);
    sysToast('操作日志已导出为 CSV');
  } catch (e) {
    sysToast(e.message || '导出失败', true);
  }
}

// ============================================================================
// 5. 页面重载映射（页面级 401 提示中的「刷新」按钮）
// ============================================================================

var SYS_PAGE_LOADERS = {};

function sysReloadPage(page) {
  const loader = SYS_PAGE_LOADERS[page];
  if (typeof loader !== 'function') return;
  Promise.resolve()
    .then(function () { return loader(); })
    .catch(function (e) { sysHandleError(page, e); });
}

// ============================================================================
// 6. 通知设置页（#view-notifications）
// ============================================================================

function sysChannelTypeLabel(type) {
  const found = SYS_CHANNEL_TYPES.find(function (item) { return item.value === type; });
  return found ? found.label : (type || '-');
}

function sysChannelTypeBadge(type) {
  const tones = { webhook: 'indigo', feishu: 'emerald', serverchan: 'amber', email: 'slate', sms: 'rose' };
  return sysBadge(sysChannelTypeLabel(type), tones[type] || 'slate');
}

function sysChannelName(id) {
  if (id === null || id === undefined || id === '') return '渠道已删除';
  const found = (sysState.notifications.channels || []).find(function (item) {
    return sysNum(item.id) === sysNum(id);
  });
  return found ? (found.name || ('#' + sysNum(id))) : ('#' + sysNum(id));
}

function sysChannelConfigSummary(channel) {
  let cfg = {};
  const raw = channel.config_json;
  if (raw && typeof raw === 'object') {
    cfg = raw;
  } else if (raw) {
    try {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed === 'object') cfg = parsed;
    } catch (_) {
      return '配置 JSON 解析失败';
    }
  }
  const type = channel.channel_type;
  if (type === 'webhook') {
    return 'URL: ' + (cfg.url || '未配置') + ' · Secret: ' + sysMaskSecret(cfg.secret);
  }
  if (type === 'feishu') {
    return 'Webhook: ' + (cfg.webhook_url || '未配置');
  }
  if (type === 'serverchan') {
    return 'SendKey: ' + sysMaskSecret(cfg.send_key);
  }
  if (type === 'email' || type === 'sms') {
    return '网关: ' + (cfg.gateway_url || '未配置');
  }
  const keys = Object.keys(cfg);
  return keys.length ? '配置字段: ' + keys.join(', ') : '未配置';
}

function sysEventLabel(type) {
  const found = (sysState.notifications.events || []).find(function (event) {
    return event.event_type === type;
  });
  if (found && found.label) return found.label;
  return SYS_EVENT_LABELS[type] || type || '-';
}

function sysEventOptions() {
  const events = sysState.notifications.events || [];
  return events.length ? events : SYS_NOTIFICATION_EVENTS_FALLBACK;
}

function sysSelectOptions(items, current) {
  return Object.keys(items).map(function (value) {
    return '<option value="' + sysEscape(value) + '"' + (value === current ? ' selected' : '') + '>' +
      sysEscape(items[value]) + ' (' + sysEscape(value) + ')</option>';
  }).join('');
}

function sysRenderNotificationsSkeleton() {
  const view = document.getElementById('view-notifications');
  if (!view) return;
  view.innerHTML =
    '<div id="sys-notif-root" class="space-y-4">' +
    sysAuthBannerHtml('notifications') +
    '<div class="grid grid-cols-2 lg:grid-cols-4 gap-4">' +
    sysStatCard('通知渠道', 'sys-notif-stat-channels', 'Webhook / 飞书 / ServerChan / 邮件 / 短信', 'text-slate-900') +
    sysStatCard('已启用渠道', 'sys-notif-stat-enabled', '参与事件推送的渠道', 'text-emerald-600') +
    sysStatCard('事件规则', 'sys-notif-stat-rules', '渠道 + 事件类型唯一匹配', 'text-indigo-600') +
    sysStatCard('近 30 条失败', 'sys-notif-stat-failed', 'notification_logs.status = failed', 'text-rose-600') +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm px-6 py-4 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h2 class="text-sm font-bold text-slate-900">通知设置</h2>' +
    '<p class="text-xs text-slate-500 mt-1">配置通知渠道与事件规则；事件触发时按规则发送，发送结果记录在发送日志中。</p>' +
    '</div>' +
    '<div class="flex gap-2">' +
    '<button onclick="sysNotificationRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新全部</button>' +
    '<button onclick="sysNotificationOpenChannelModal()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">+ 新建渠道</button>' +
    '</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">通知渠道</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">配置以 config_json 保存；Secret / Key 在列表中脱敏展示。</p>' +
    '</div>' +
    '<button onclick="sysNotificationRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新</button>' +
    '</div>' +
    '<div id="sys-notif-channels" class="p-5 grid md:grid-cols-2 xl:grid-cols-3 gap-3">' +
    '<div data-sys-loading-block class="md:col-span-2 xl:col-span-3 py-8 text-center text-xs text-slate-400">正在加载通知渠道…</div>' +
    '</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">事件规则</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">命中「渠道 + 事件类型」时，按模板渲染并发送通知。</p>' +
    '</div>' +
    '<button onclick="sysNotificationOpenRuleModal()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">+ 新增规则</button>' +
    '</div>' +
    '<div id="sys-notif-events" data-sys-loading-block class="px-6 py-3 border-b border-slate-100 bg-slate-50/60">' +
    '<div class="text-xs text-slate-400">正在加载事件说明…</div>' +
    '</div>' +
    '<div class="overflow-x-auto">' +
    '<table class="w-full text-left">' +
    '<thead><tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">' +
    '<th class="py-3 px-4">事件类型</th>' +
    '<th class="py-3 px-4">渠道</th>' +
    '<th class="py-3 px-4">启用</th>' +
    '<th class="py-3 px-4">模板</th>' +
    '<th class="py-3 px-4">操作</th>' +
    '</tr></thead>' +
    '<tbody id="sys-notif-rules-tbody" data-sys-loading-body data-colspan="5" class="divide-y divide-slate-100 text-sm">' +
    '<tr><td colspan="5" class="py-10 px-4 text-center text-slate-400 text-xs">正在加载事件规则…</td></tr>' +
    '</tbody>' +
    '</table>' +
    '</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">发送日志</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">最近 30 条通知发送记录（GET /api/notifications/logs?limit=30）。</p>' +
    '</div>' +
    '<button onclick="sysNotificationRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新日志</button>' +
    '</div>' +
    '<div class="overflow-x-auto">' +
    '<table class="w-full text-left">' +
    '<thead><tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">' +
    '<th class="py-3 px-4">事件</th>' +
    '<th class="py-3 px-4">渠道</th>' +
    '<th class="py-3 px-4">标题</th>' +
    '<th class="py-3 px-4">状态</th>' +
    '<th class="py-3 px-4">错误</th>' +
    '<th class="py-3 px-4">时间</th>' +
    '<th class="py-3 px-4">操作</th>' +
    '</tr></thead>' +
    '<tbody id="sys-notif-logs-tbody" data-sys-loading-body data-colspan="7" class="divide-y divide-slate-100 text-sm">' +
    '<tr><td colspan="7" class="py-10 px-4 text-center text-slate-400 text-xs">正在加载通知发送日志…</td></tr>' +
    '</tbody>' +
    '</table>' +
    '</div>' +
    '</div>' +
    '</div>';
}

function sysNotificationStatusBadge(status) {
  const map = {
    sent: { label: '已发送', tone: 'emerald' },
    failed: { label: '发送失败', tone: 'rose' },
    pending: { label: '待发送', tone: 'amber' }
  };
  const item = map[status] || { label: status || '未知', tone: 'slate' };
  return sysBadge(item.label, item.tone);
}

function sysRenderNotificationStats() {
  const st = sysState.notifications;
  const channels = st.channels || [];
  const rules = st.rules || [];
  const logs = st.logs || [];
  sysSetText('sys-notif-stat-channels', channels.length);
  sysSetText('sys-notif-stat-enabled', channels.filter(function (c) { return !!c.enabled; }).length);
  sysSetText('sys-notif-stat-rules', rules.length);
  sysSetText('sys-notif-stat-failed', logs.filter(function (log) { return log.status === 'failed'; }).length);
}

function sysRenderNotificationChannels() {
  const wrap = document.getElementById('sys-notif-channels');
  if (!wrap) return;
  const channels = sysState.notifications.channels || [];
  if (!channels.length) {
    wrap.innerHTML = '<div class="md:col-span-2 xl:col-span-3">' +
      sysEmptyBlock('尚未配置通知渠道',
        '创建 Webhook / 飞书 / ServerChan / 邮件 / 短信渠道后，事件触发时会按规则推送。',
        '<button onclick="sysNotificationOpenChannelModal()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">+ 新建渠道</button>') +
      '</div>';
    return;
  }
  wrap.innerHTML = channels.map(function (channel) {
    const id = sysNum(channel.id);
    const summary = sysChannelConfigSummary(channel);
    const enabled = !!channel.enabled;
    return '<div class="rounded-xl border border-slate-200 p-4 hover:border-indigo-200 transition">' +
      '<div class="flex items-start justify-between gap-3">' +
      '<div class="min-w-0">' +
      '<div class="text-sm font-bold text-slate-900 truncate" title="' + sysEscape(channel.name || '') + '">' + sysEscape(channel.name || '-') + '</div>' +
      '<div class="mt-1.5 flex flex-wrap items-center gap-1.5">' +
      sysChannelTypeBadge(channel.channel_type) +
      sysBadge(enabled ? '已启用' : '已停用', enabled ? 'emerald' : 'slate') +
      '</div>' +
      '</div>' +
      '<label class="inline-flex items-center gap-1.5 text-[11px] text-slate-500 shrink-0 cursor-pointer">' +
      '<input type="checkbox" ' + (enabled ? 'checked' : '') + ' onchange="sysNotificationToggleChannel(' + id + ', this.checked)" class="w-4 h-4 rounded border-slate-300 text-indigo-600" />' +
      '<span>启用</span>' +
      '</label>' +
      '</div>' +
      '<div class="mt-3 text-[11px] text-slate-500 font-mono leading-relaxed break-all min-h-[32px]">' + sysEscape(summary) + '</div>' +
      '<div class="mt-3 flex flex-wrap gap-1.5">' +
      '<button id="sys-channel-test-' + id + '" onclick="sysNotificationTestChannel(' + id + ')" class="px-2.5 py-1 rounded-lg bg-emerald-50 hover:bg-emerald-100 text-emerald-700 text-[11px] font-medium">测试</button>' +
      '<button onclick="sysNotificationOpenChannelModal(' + id + ')" class="px-2.5 py-1 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-[11px] font-medium">编辑</button>' +
      '<button onclick="sysNotificationDeleteChannel(' + id + ')" class="px-2.5 py-1 rounded-lg bg-rose-50 hover:bg-rose-100 text-rose-600 text-[11px] font-medium">删除</button>' +
      '</div>' +
      '</div>';
  }).join('');
}

function sysRenderNotificationEvents() {
  const el = document.getElementById('sys-notif-events');
  if (!el) return;
  const st = sysState.notifications;
  const events = sysEventOptions();
  const chips = events.map(function (event) {
    return '<span class="inline-flex items-center gap-1 px-2 py-1 rounded-lg bg-white border border-slate-200 text-[11px] text-slate-600">' +
      '<span class="font-semibold">' + sysEscape(event.label || '-') + '</span>' +
      '<span class="font-mono text-slate-400">' + sysEscape(event.event_type || '-') + '</span>' +
      '</span>';
  }).join('');
  const note = st.eventsError
    ? '<span class="text-rose-500">事件说明接口暂不可用，以上为契约内置事件列表。</span>'
    : '模板占位符：<span class="font-mono">{event}</span> <span class="font-mono">{title}</span> <span class="font-mono">{content}</span> <span class="font-mono">{time}</span>；规则按「渠道 + 事件类型」唯一匹配。';
  el.innerHTML = '<div class="flex flex-wrap items-center gap-1.5">' + chips + '</div>' +
    '<div class="text-[11px] text-slate-400 mt-2">' + note + '</div>';
}

function sysRenderNotificationRules() {
  const tbody = document.getElementById('sys-notif-rules-tbody');
  if (!tbody) return;
  const rules = sysState.notifications.rules || [];
  if (!rules.length) {
    tbody.innerHTML = sysTableEmpty(5, '暂无事件规则', '点击「+ 新增规则」把渠道绑定到事件类型，事件触发时才会发送通知。');
    return;
  }
  tbody.innerHTML = rules.map(function (rule) {
    const id = sysNum(rule.id);
    const enabled = !!rule.enabled;
    return '<tr class="hover:bg-slate-50/80 align-top">' +
      '<td class="py-3 px-4">' +
      '<div class="text-xs font-semibold text-slate-800">' + sysEscape(sysEventLabel(rule.event_type)) + '</div>' +
      '<div class="text-[11px] font-mono text-slate-400 mt-0.5">' + sysEscape(rule.event_type || '-') + '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-700">' + sysEscape(rule.channel_name || sysChannelName(rule.channel_id)) + '</td>' +
      '<td class="py-3 px-4">' +
      '<label class="inline-flex items-center gap-1.5 text-[11px] text-slate-500 cursor-pointer">' +
      '<input type="checkbox" ' + (enabled ? 'checked' : '') + ' onchange="sysNotificationToggleRule(' + id + ', this.checked)" class="w-4 h-4 rounded border-slate-300 text-indigo-600" />' +
      '<span>' + (enabled ? '已启用' : '已停用') + '</span>' +
      '</label>' +
      '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-500 max-w-[260px]">' +
      '<div class="truncate" title="' + sysEscape(rule.template || '') + '">' + sysEscape(sysTruncate(rule.template || '-', 46)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4">' +
      '<div class="flex flex-wrap gap-1.5">' +
      '<button onclick="sysNotificationOpenRuleModal(' + id + ')" class="px-2.5 py-1 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-[11px] font-medium">编辑</button>' +
      '<button onclick="sysNotificationDeleteRule(' + id + ')" class="px-2.5 py-1 rounded-lg bg-rose-50 hover:bg-rose-100 text-rose-600 text-[11px] font-medium">删除</button>' +
      '</div>' +
      '</td>' +
      '</tr>';
  }).join('');
}

function sysRenderNotificationLogs() {
  const tbody = document.getElementById('sys-notif-logs-tbody');
  if (!tbody) return;
  const logs = sysState.notifications.logs || [];
  if (!logs.length) {
    tbody.innerHTML = sysTableEmpty(7, '暂无通知发送记录', '触发一次通知事件或点击渠道「测试」后，发送结果会记录在这里。');
    return;
  }
  tbody.innerHTML = logs.map(function (log) {
    const id = sysNum(log.id);
    return '<tr class="hover:bg-slate-50/80 align-top">' +
      '<td class="py-3 px-4">' +
      '<div class="text-xs font-semibold text-slate-800">' + sysEscape(sysEventLabel(log.event_type)) + '</div>' +
      '<div class="text-[11px] font-mono text-slate-400 mt-0.5">' + sysEscape(log.event_type || '-') + '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-700">' + sysEscape(log.channel_name || sysChannelName(log.channel_id)) + '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-700 max-w-[200px]">' +
      '<div class="truncate" title="' + sysEscape(log.title || '') + '">' + sysEscape(sysTruncate(log.title || '-', 32)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4">' + sysNotificationStatusBadge(log.status) + '</td>' +
      '<td class="py-3 px-4 text-xs text-rose-500 max-w-[220px]">' +
      '<div class="truncate" title="' + sysEscape(log.error_message || '') + '">' + sysEscape(sysTruncate(log.error_message || '-', 36)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-[11px] font-mono text-slate-400 whitespace-nowrap">' + sysEscape(sysFmtTime(log.created_at)) + '</td>' +
      '<td class="py-3 px-4">' +
      '<button onclick="sysNotificationRetryLog(' + id + ')" class="px-2.5 py-1 rounded-lg bg-indigo-50 hover:bg-indigo-100 text-indigo-700 text-[11px] font-medium">重试</button>' +
      '</td>' +
      '</tr>';
  }).join('');
}

async function sysNotificationFetchChannels() {
  const list = await sysApi('/api/notifications/channels');
  sysState.notifications.channels = sysAsArray(list);
  sysRenderNotificationChannels();
  sysRenderNotificationStats();
  sysSetText('badge-notifications', sysState.notifications.channels.length);
}

async function sysNotificationFetchRules() {
  const list = await sysApi('/api/notifications/rules');
  sysState.notifications.rules = sysAsArray(list);
  sysRenderNotificationRules();
  sysRenderNotificationStats();
}

async function sysNotificationFetchEvents() {
  const st = sysState.notifications;
  try {
    const list = await sysApi('/api/notifications/events');
    st.events = sysAsArray(list);
    st.eventsError = false;
  } catch (e) {
    st.events = [];
    st.eventsError = true;
    if (sysIsAuthError(e)) {
      sysRenderNotificationEvents();
      throw e;
    }
    sysRenderNotificationEvents();
    return;
  }
  sysRenderNotificationEvents();
}

async function sysNotificationFetchLogs() {
  const list = await sysApi('/api/notifications/logs?limit=30');
  sysState.notifications.logs = sysAsArray(list);
  sysRenderNotificationLogs();
  sysRenderNotificationStats();
}

async function sysNotificationRefreshAll() {
  const st = sysState.notifications;
  if (st.loading) return;
  st.loading = true;
  try {
    const results = await Promise.allSettled([
      sysNotificationFetchChannels(),
      sysNotificationFetchRules(),
      sysNotificationFetchEvents(),
      sysNotificationFetchLogs()
    ]);
    sysReportResults('notifications', results);
    if (results[0].status === 'rejected' && !sysIsAuthError(results[0].reason)) {
      sysErrorBlock('sys-notif-channels', '通知渠道加载失败', results[0].reason, 'md:col-span-2 xl:col-span-3');
    }
    if (results[1].status === 'rejected' && !sysIsAuthError(results[1].reason)) {
      sysErrorTable('sys-notif-rules-tbody', '事件规则加载失败', results[1].reason);
    }
    if (results[3].status === 'rejected' && !sysIsAuthError(results[3].reason)) {
      sysErrorTable('sys-notif-logs-tbody', '通知发送日志加载失败', results[3].reason);
    }
    if (results.some(function (r) { return r.status === 'fulfilled'; })) {
      sysHideAuth('notifications');
    }
  } catch (e) {
    sysHandleError('notifications', e);
  } finally {
    st.loading = false;
  }
}

async function loadSysNotifications() {
  try {
    const st = sysState.notifications;
    if (!st.ready || !document.getElementById('sys-notif-root')) {
      sysRenderNotificationsSkeleton();
      st.ready = true;
    }
    await sysNotificationRefreshAll();
  } catch (e) {
    sysHandleError('notifications', e);
  }
}

async function sysNotificationToggleChannel(id, enabled) {
  try {
    await sysApi('/api/notifications/channels/' + id, sysJsonOptions('PUT', { enabled: !!enabled }));
    sysToast(enabled ? '通知渠道已启用' : '通知渠道已停用');
    await sysNotificationFetchChannels();
  } catch (e) {
    sysRenderNotificationChannels();
    sysHandleError('notifications', e);
  }
}

async function sysNotificationTestChannel(id) {
  const btn = document.getElementById('sys-channel-test-' + id);
  if (btn) {
    btn.disabled = true;
    btn.textContent = '发送中…';
    btn.classList.add('opacity-60');
  }
  try {
    const res = await sysApi('/api/notifications/channels/' + id + '/test', { method: 'POST' });
    if (res && res.status === 'sent') {
      sysToast('测试通知已发送' + (res.response ? '：' + res.response : ''));
    } else {
      sysToast((res && (res.error_message || res.response)) || '测试通知未发送成功', true);
    }
    await sysNotificationFetchLogs();
  } catch (e) {
    sysHandleError('notifications', e);
  } finally {
    if (btn && document.body.contains(btn)) {
      btn.disabled = false;
      btn.textContent = '测试';
      btn.classList.remove('opacity-60');
    }
  }
}

function sysNotificationOpenChannelModal(id) {
  const st = sysState.notifications;
  const channel = id ? (st.channels || []).find(function (item) { return sysNum(item.id) === sysNum(id); }) : null;
  let cfg = {};
  if (channel && channel.config_json) {
    if (typeof channel.config_json === 'object') {
      cfg = channel.config_json;
    } else {
      try {
        const parsed = JSON.parse(channel.config_json);
        if (parsed && typeof parsed === 'object') cfg = parsed;
      } catch (_) {
        cfg = {};
      }
    }
  }
  st.editingConfig = cfg;
  const currentType = channel ? (channel.channel_type || 'webhook') : 'webhook';
  const typeOptions = SYS_CHANNEL_TYPES.map(function (type) {
    return '<option value="' + sysEscape(type.value) + '">' + sysEscape(type.label) + ' (' + sysEscape(type.value) + ')</option>';
  }).join('');
  sysEnsureModal('sys-notification-modal',
    sysModalHeader(channel ? '编辑通知渠道' : '新建通知渠道', 'sys-notification-modal') +
    '<input type="hidden" id="sys-channel-id" value="' + (channel ? sysNum(channel.id) : '') + '" />' +
    '<div class="space-y-3 text-xs">' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">渠道名称 *</label>' +
    '<input id="sys-channel-name" type="text" value="' + sysEscape(channel ? (channel.name || '') : '') + '" placeholder="例如：运营告警群" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm" />' +
    '</div>' +
    '<div class="grid grid-cols-2 gap-3">' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">渠道类型</label>' +
    '<select id="sys-channel-type" onchange="sysChannelTypeChanged()" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm">' + typeOptions + '</select>' +
    '</div>' +
    '<div class="flex items-end pb-2">' +
    '<label class="inline-flex items-center gap-2 cursor-pointer">' +
    '<input id="sys-channel-enabled" type="checkbox" ' + (channel ? (channel.enabled ? 'checked' : '') : 'checked') + ' class="w-4 h-4 rounded border-slate-300 text-indigo-600" />' +
    '<span class="text-xs font-medium text-slate-700">启用该渠道</span>' +
    '</label>' +
    '</div>' +
    '</div>' +
    '<div id="sys-channel-config-fields" class="space-y-3"></div>' +
    '<div class="text-[11px] text-slate-400">配置会以 JSON 字符串保存到 config_json；密钥字段仅用于后端调用，列表中会脱敏展示。</div>' +
    '</div>' +
    '<div class="flex justify-end gap-2 pt-2">' +
    sysModalCancelButton('sys-notification-modal') +
    '<button onclick="sysNotificationSaveChannel()" class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">保存渠道</button>' +
    '</div>'
  );
  const typeEl = document.getElementById('sys-channel-type');
  if (typeEl) {
    typeEl.value = currentType;
    if (!typeEl.value) typeEl.value = 'webhook';
  }
  sysRenderChannelConfigFields(typeEl ? typeEl.value : 'webhook', cfg);
  sysOpenModal('sys-notification-modal');
}

function sysRenderChannelConfigFields(type, cfg) {
  const container = document.getElementById('sys-channel-config-fields');
  if (!container) return;
  const fields = SYS_CHANNEL_CONFIG_FIELDS[type] || [];
  const safeCfg = cfg || {};
  if (!fields.length) {
    container.innerHTML = '<div class="text-xs text-slate-400 rounded-lg border border-dashed border-slate-200 px-3 py-2">该渠道类型无需额外配置。</div>';
    return;
  }
  container.innerHTML = fields.map(function (field) {
    const value = safeCfg[field.key] === undefined || safeCfg[field.key] === null ? '' : safeCfg[field.key];
    return '<div>' +
      '<label class="block text-slate-700 font-semibold mb-1">' + sysEscape(field.label) +
      (field.required ? ' <span class="text-rose-500">*</span>' : '') + '</label>' +
      '<input data-cfg-key="' + sysEscape(field.key) + '" type="' + (field.type === 'password' ? 'password' : 'text') + '"' +
      ' value="' + sysEscape(value) + '"' +
      ' placeholder="' + sysEscape(field.placeholder || '') + '"' +
      ' class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono" />' +
      '</div>';
  }).join('');
}

function sysChannelTypeChanged() {
  const st = sysState.notifications;
  const collected = sysCollectChannelConfig();
  st.editingConfig = Object.assign({}, st.editingConfig || {}, collected);
  const typeEl = document.getElementById('sys-channel-type');
  sysRenderChannelConfigFields(typeEl ? typeEl.value : 'webhook', st.editingConfig);
}

function sysCollectChannelConfig() {
  const out = {};
  const container = document.getElementById('sys-channel-config-fields');
  if (!container) return out;
  container.querySelectorAll('[data-cfg-key]').forEach(function (el) {
    out[el.getAttribute('data-cfg-key')] = el.value;
  });
  return out;
}

async function sysNotificationSaveChannel() {
  const id = sysVal('sys-channel-id');
  const name = sysVal('sys-channel-name');
  const channelType = sysVal('sys-channel-type') || 'webhook';
  const enabledEl = document.getElementById('sys-channel-enabled');
  const enabled = enabledEl ? enabledEl.checked : false;
  if (!name) return sysToast('请填写渠道名称', true);
  const fields = SYS_CHANNEL_CONFIG_FIELDS[channelType] || [];
  const collected = sysCollectChannelConfig();
  const cfg = Object.assign({}, sysState.notifications.editingConfig || {}, collected);
  for (let i = 0; i < fields.length; i += 1) {
    const field = fields[i];
    const value = cfg[field.key] === undefined || cfg[field.key] === null ? '' : String(cfg[field.key]).trim();
    cfg[field.key] = value;
    if (field.required && !value) {
      return sysToast('请填写' + field.label, true);
    }
  }
  const payload = {
    name: name,
    channel_type: channelType,
    config_json: JSON.stringify(cfg),
    enabled: enabled
  };
  try {
    if (id) {
      await sysApi('/api/notifications/channels/' + id, sysJsonOptions('PUT', payload));
    } else {
      await sysApi('/api/notifications/channels', sysJsonOptions('POST', payload));
    }
    sysState.notifications.editingConfig = {};
    sysCloseModal('sys-notification-modal');
    sysToast(id ? '通知渠道已更新' : '通知渠道已创建');
    await sysNotificationRefreshAll();
  } catch (e) {
    if (sysIsAuthError(e)) sysCloseModal('sys-notification-modal');
    sysHandleError('notifications', e);
  }
}

async function sysNotificationDeleteChannel(id) {
  const channel = (sysState.notifications.channels || []).find(function (item) { return sysNum(item.id) === sysNum(id); });
  const name = channel ? channel.name : String(id);
  if (!window.confirm('确认删除通知渠道「' + name + '」？绑定到该渠道的事件规则将不再发送通知。')) return;
  try {
    await sysApi('/api/notifications/channels/' + id, { method: 'DELETE' });
    sysToast('通知渠道已删除');
    await sysNotificationRefreshAll();
  } catch (e) {
    sysHandleError('notifications', e);
  }
}

function sysNotificationOpenRuleModal(id) {
  const st = sysState.notifications;
  const rule = id ? (st.rules || []).find(function (item) { return sysNum(item.id) === sysNum(id); }) : null;
  const channels = st.channels || [];
  if (!channels.length) {
    return sysToast('请先创建至少一个通知渠道', true);
  }
  const events = sysEventOptions();
  const channelId = rule ? sysNum(rule.channel_id) : sysNum(channels[0].id);
  const eventType = rule ? (rule.event_type || '') : (events[0] ? events[0].event_type : '');
  const channelOptions = channels.map(function (channel) {
    return '<option value="' + sysNum(channel.id) + '"' + (sysNum(channel.id) === channelId ? ' selected' : '') + '>' +
      sysEscape(channel.name || ('#' + sysNum(channel.id))) +
      ' (' + sysEscape(sysChannelTypeLabel(channel.channel_type)) + ')</option>';
  }).join('');
  const eventOptions = events.map(function (event) {
    return '<option value="' + sysEscape(event.event_type) + '"' + (event.event_type === eventType ? ' selected' : '') + '>' +
      sysEscape(event.label || event.event_type) + ' (' + sysEscape(event.event_type) + ')</option>';
  }).join('');
  const template = rule ? (rule.template || '') : '【{event}】{title}\n{content}\n时间：{time}';
  sysEnsureModal('sys-notification-rule-modal',
    sysModalHeader(rule ? '编辑事件规则' : '新增事件规则', 'sys-notification-rule-modal') +
    '<input type="hidden" id="sys-notification-rule-id" value="' + (rule ? sysNum(rule.id) : '') + '" />' +
    '<div class="space-y-3 text-xs">' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">通知渠道</label>' +
    '<select id="sys-notification-rule-channel" ' + (rule ? 'disabled' : '') + ' class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm disabled:bg-slate-100">' + channelOptions + '</select>' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">事件类型</label>' +
    '<select id="sys-notification-rule-event" ' + (rule ? 'disabled' : '') + ' class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm disabled:bg-slate-100">' + eventOptions + '</select>' +
    '</div>' +
    '<label class="flex items-center gap-2 rounded-lg border border-slate-200 bg-slate-50 px-3 py-2.5 cursor-pointer">' +
    '<input id="sys-notification-rule-enabled" type="checkbox" ' + (rule ? (rule.enabled ? 'checked' : '') : 'checked') + ' class="w-4 h-4 rounded border-slate-300 text-indigo-600" />' +
    '<span class="text-xs font-medium text-slate-700">启用该规则</span>' +
    '</label>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">消息模板</label>' +
    '<textarea id="sys-notification-rule-template" rows="4" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-xs font-mono" placeholder="使用 {event} / {title} / {content} / {time} 占位符">' + sysEscape(template) + '</textarea>' +
    '</div>' +
    '<div class="text-[11px] text-slate-400">编辑已有规则时，渠道与事件类型不可修改（PUT 仅支持 enabled / template）。</div>' +
    '</div>' +
    '<div class="flex justify-end gap-2 pt-2">' +
    sysModalCancelButton('sys-notification-rule-modal') +
    '<button onclick="sysNotificationSaveRule()" class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">保存规则</button>' +
    '</div>'
  );
  sysOpenModal('sys-notification-rule-modal');
}

async function sysNotificationSaveRule() {
  const id = sysVal('sys-notification-rule-id');
  const enabledEl = document.getElementById('sys-notification-rule-enabled');
  const enabled = enabledEl ? enabledEl.checked : true;
  const templateEl = document.getElementById('sys-notification-rule-template');
  const template = templateEl ? templateEl.value : '';
  try {
    if (id) {
      await sysApi('/api/notifications/rules/' + id, sysJsonOptions('PUT', {
        enabled: enabled,
        template: template
      }));
    } else {
      const channelId = parseInt(sysVal('sys-notification-rule-channel'), 10);
      const eventType = sysVal('sys-notification-rule-event');
      if (!Number.isFinite(channelId)) return sysToast('请选择通知渠道', true);
      if (!eventType) return sysToast('请选择事件类型', true);
      await sysApi('/api/notifications/rules', sysJsonOptions('POST', {
        channel_id: channelId,
        event_type: eventType,
        enabled: enabled,
        template: template
      }));
    }
    sysCloseModal('sys-notification-rule-modal');
    sysToast(id ? '事件规则已更新' : '事件规则已创建');
    await sysNotificationFetchRules();
  } catch (e) {
    if (sysIsAuthError(e)) sysCloseModal('sys-notification-rule-modal');
    sysHandleError('notifications', e);
  }
}

async function sysNotificationToggleRule(id, enabled) {
  try {
    await sysApi('/api/notifications/rules/' + id, sysJsonOptions('PUT', { enabled: !!enabled }));
    sysToast(enabled ? '事件规则已启用' : '事件规则已停用');
    await sysNotificationFetchRules();
  } catch (e) {
    sysRenderNotificationRules();
    sysHandleError('notifications', e);
  }
}

async function sysNotificationDeleteRule(id) {
  if (!window.confirm('确认删除这条事件规则？删除后该事件不会再通过此规则发送通知。')) return;
  try {
    await sysApi('/api/notifications/rules/' + id, { method: 'DELETE' });
    sysToast('事件规则已删除');
    await sysNotificationFetchRules();
  } catch (e) {
    sysHandleError('notifications', e);
  }
}

async function sysNotificationRetryLog(id) {
  try {
    const res = await sysApi('/api/notifications/logs/' + id + '/retry', { method: 'POST' });
    const status = res && res.status;
    if (status === 'sent') {
      sysToast((res && res.message) || '重试成功，通知已发送');
    } else {
      sysToast((res && (res.message || res.error_message)) || '重试完成，但通知仍未发送成功', true);
    }
    await sysNotificationFetchLogs();
  } catch (e) {
    sysHandleError('notifications', e);
  }
}

// ============================================================================
// 7. 系统设置页（#view-system）
// ============================================================================

var SYS_SYSTEM_COUNT_DEFS = [
  { key: 'accounts', label: '闲鱼账号' },
  { key: 'items', label: '闲鱼商品' },
  { key: 'products', label: '卡密商品' },
  { key: 'orders', label: '订单' },
  { key: 'conversations', label: '会话' },
  { key: 'deliveries', label: '发货记录' },
  { key: 'scheduled_tasks', label: '定时任务' },
  { key: 'unread_messages', label: '未读消息' }
];

function sysInfoTile(label, value, sub) {
  return '<div class="rounded-xl border border-slate-200 bg-slate-50/60 p-4">' +
    '<div class="text-[11px] font-semibold text-slate-500">' + sysEscape(label) + '</div>' +
    '<div class="mt-1 text-base font-bold text-slate-900 font-mono break-all">' + sysEscape(value) + '</div>' +
    (sub ? '<div class="text-[11px] text-slate-400 mt-0.5">' + sysEscape(sub) + '</div>' : '') +
    '</div>';
}

function sysRenderSystemSkeleton() {
  const view = document.getElementById('view-system');
  if (!view) return;
  view.innerHTML =
    '<div id="sys-system-root" class="space-y-4">' +
    sysAuthBannerHtml('system') +
    '<div id="sys-system-info-card" class="bg-white rounded-xl border border-slate-200 shadow-sm">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">系统信息</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">版本、运行时长、数据库大小、服务器时间与各实体数量。</p>' +
    '</div>' +
    '<button onclick="sysSystemRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新</button>' +
    '</div>' +
    '<div id="sys-system-info-body" data-sys-loading-block class="p-6"><div class="py-6 text-center text-xs text-slate-400">正在加载系统信息…</div></div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm px-6 py-4 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">系统设置</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">按 group_name 分组展示；按 value_type 渲染输入控件，保存时提交全部可见设置项。</p>' +
    '</div>' +
    '<div class="flex gap-2">' +
    '<button onclick="sysSystemRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">重新加载</button>' +
    '<button id="sys-system-save-btn" onclick="sysSystemSaveSettings()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">保存设置</button>' +
    '</div>' +
    '</div>' +

    '<div id="sys-settings-groups" class="space-y-4">' +
    '<div data-sys-loading-block class="bg-white rounded-xl border border-slate-200 shadow-sm p-10 text-center text-xs text-slate-400">正在加载系统设置…</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">数据维护</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">备份 SQLite 数据库文件，或按保留天数清理历史日志与发货记录。</p>' +
    '</div>' +
    '<button id="sys-system-backup-btn" onclick="sysSystemBackup()" class="px-3 py-1.5 rounded-lg bg-emerald-50 hover:bg-emerald-100 text-emerald-700 text-xs font-semibold">立即备份</button>' +
    '</div>' +
    '<div class="p-6 space-y-5">' +
    '<div>' +
    '<div class="text-xs font-bold text-slate-600 mb-2">备份列表</div>' +
    '<div class="border border-slate-200 rounded-xl overflow-hidden">' +
    '<table class="w-full text-left">' +
    '<thead><tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">' +
    '<th class="py-3 px-4">文件</th>' +
    '<th class="py-3 px-4">大小</th>' +
    '<th class="py-3 px-4">创建时间</th>' +
    '</tr></thead>' +
    '<tbody id="sys-system-backups-tbody" data-sys-loading-body data-colspan="3" class="divide-y divide-slate-100 text-sm">' +
    '<tr><td colspan="3" class="py-8 px-4 text-center text-slate-400 text-xs">正在加载备份列表…</td></tr>' +
    '</tbody>' +
    '</table>' +
    '</div>' +
    '</div>' +
    '<div class="rounded-xl border border-rose-100 bg-rose-50/50 p-4 flex flex-wrap items-end justify-between gap-3">' +
    '<div>' +
    '<div class="text-xs font-bold text-rose-700">清理历史数据</div>' +
    '<div class="text-[11px] text-rose-500 mt-0.5">删除早于指定天数的操作日志、通知日志与发货记录，该操作不可恢复。</div>' +
    '</div>' +
    '<div class="flex items-center gap-2">' +
    '<input id="sys-system-cleanup-days" type="number" min="1" value="30" class="w-24 rounded-lg border border-slate-300 px-2 py-1.5 text-xs font-mono" />' +
    '<span class="text-xs text-slate-500">天前</span>' +
    '<button onclick="sysSystemCleanup()" class="px-3 py-1.5 rounded-lg bg-rose-600 hover:bg-rose-700 text-white text-xs font-semibold">执行清理</button>' +
    '</div>' +
    '</div>' +
    '</div>' +
    '</div>' +
    '</div>';
}

function sysRenderSystemInfo() {
  const card = document.getElementById('sys-system-info-card');
  if (!card) return;
  const st = sysState.system;
  const info = st.info || {};
  const counts = info.counts || {};
  const countTiles = SYS_SYSTEM_COUNT_DEFS.map(function (def) {
    return '<div class="rounded-xl border border-slate-200 px-4 py-3">' +
      '<div class="text-[11px] text-slate-500">' + sysEscape(def.label) + '</div>' +
      '<div class="mt-0.5 text-lg font-bold text-indigo-600">' + sysNum(counts[def.key]) + '</div>' +
      '</div>';
  }).join('');
  card.innerHTML =
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">系统信息</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">GET /api/system/info · 服务器时间与数据实体统计</p>' +
    '</div>' +
    '<button onclick="sysSystemRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新</button>' +
    '</div>' +
    '<div id="sys-system-info-body" class="p-6 space-y-5">' +
    '<div class="grid sm:grid-cols-2 xl:grid-cols-4 gap-3">' +
    sysInfoTile('版本', info.version || '-', '应用版本号') +
    sysInfoTile('运行时长', sysFmtDuration(info.uptime_seconds), '累计 ' + sysNum(info.uptime_seconds) + ' 秒') +
    sysInfoTile('数据库大小', sysFmtBytes(info.db_size_bytes), '共 ' + sysNum(info.db_size_bytes) + ' 字节') +
    sysInfoTile('服务器时间', sysFmtTime(info.server_time), info.timezone ? ('时区 ' + info.timezone) : '') +
    '</div>' +
    '<div>' +
    '<div class="text-xs font-bold text-slate-600 mb-2">数据实体数量</div>' +
    '<div class="grid grid-cols-2 sm:grid-cols-4 gap-2">' + countTiles + '</div>' +
    '</div>' +
    '</div>';
}

function sysSettingField(setting) {
  const key = setting.key;
  const type = setting.value_type || 'string';
  const value = setting.value === null || setting.value === undefined ? '' : String(setting.value);
  const label = setting.label || key;
  const desc = setting.description || '';
  const meta = '<div class="text-[11px] text-slate-400 mt-1">' +
    (desc ? sysEscape(desc) + ' ' : '') +
    '<span class="font-mono text-slate-300">#' + sysEscape(key) + '</span>' +
    '</div>';
  if (type === 'bool') {
    const checked = value === 'true' || value === '1';
    return '<div class="flex items-start justify-between gap-4 rounded-xl border border-slate-200 bg-slate-50/60 px-4 py-3">' +
      '<div>' +
      '<div class="text-xs font-semibold text-slate-700">' + sysEscape(label) + '</div>' +
      meta +
      '</div>' +
      '<label class="inline-flex items-center gap-2 shrink-0 cursor-pointer pt-0.5">' +
      '<input type="checkbox" data-setting-key="' + sysEscape(key) + '" data-value-type="bool" ' + (checked ? 'checked' : '') + ' class="w-4 h-4 rounded border-slate-300 text-indigo-600" />' +
      '<span class="text-[11px] text-slate-500">启用</span>' +
      '</label>' +
      '</div>';
  }
  const wide = type === 'json' || key.indexOf('template') >= 0 || value.indexOf('\n') >= 0 || value.length > 60;
  const wrapClass = wide ? ' md:col-span-2' : '';
  if (type === 'json' || wide) {
    return '<div class="' + wrapClass.trim() + '">' +
      '<label class="block text-xs font-semibold text-slate-700 mb-1">' + sysEscape(label) + '</label>' +
      '<textarea data-setting-key="' + sysEscape(key) + '" data-value-type="' + sysEscape(type) + '" rows="4"' +
      ' class="w-full rounded-lg border border-slate-300 px-3 py-2 text-xs font-mono focus:outline-none focus:border-indigo-500">' +
      sysEscape(value) + '</textarea>' +
      meta +
      '</div>';
  }
  const inputType = type === 'int' ? 'number' : 'text';
  return '<div>' +
    '<label class="block text-xs font-semibold text-slate-700 mb-1">' + sysEscape(label) + '</label>' +
    '<input type="' + inputType + '" data-setting-key="' + sysEscape(key) + '" data-value-type="' + sysEscape(type) + '"' +
    ' value="' + sysEscape(value) + '"' +
    ' class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm' + (type === 'int' ? ' font-mono' : '') + ' focus:outline-none focus:border-indigo-500" />' +
    meta +
    '</div>';
}

function sysRenderSystemSettingsGroups(settings) {
  const container = document.getElementById('sys-settings-groups');
  if (!container) return;
  const groups = {};
  (settings || []).forEach(function (setting) {
    const group = setting.group_name || 'other';
    if (!groups[group]) groups[group] = [];
    groups[group].push(setting);
  });
  const order = ['basic', 'security', 'data', 'delivery', 'message'];
  const keys = order.filter(function (group) { return groups[group]; })
    .concat(Object.keys(groups).filter(function (group) { return order.indexOf(group) < 0; }).sort());
  if (!keys.length) {
    container.innerHTML = '<div class="bg-white rounded-xl border border-slate-200 shadow-sm">' +
      sysEmptyBlock('暂无系统设置项', '后端尚未初始化 system_settings 默认配置。') +
      '</div>';
    return;
  }
  container.innerHTML = keys.map(function (group) {
    const items = groups[group] || [];
    const groupLabel = SYS_SETTING_GROUP_LABELS[group] || group;
    return '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
      '<div class="px-6 py-4 border-b border-slate-200 flex items-center justify-between gap-3">' +
      '<div>' +
      '<h3 class="text-sm font-bold text-slate-900">' + sysEscape(groupLabel) + '</h3>' +
      '<p class="text-[11px] text-slate-400 mt-0.5">group_name: <span class="font-mono">' + sysEscape(group) + '</span> · ' + items.length + ' 项</p>' +
      '</div>' +
      '</div>' +
      '<div class="p-6 grid md:grid-cols-2 gap-5">' +
      items.map(sysSettingField).join('') +
      '</div>' +
      '</div>';
  }).join('');
}

function sysRenderSystemBackups() {
  const tbody = document.getElementById('sys-system-backups-tbody');
  if (!tbody) return;
  const backups = sysState.system.backups || [];
  if (!backups.length) {
    tbody.innerHTML = sysTableEmpty(3, '暂无备份文件', '点击右上角「立即备份」生成第一份 SQLite 备份。');
    return;
  }
  tbody.innerHTML = backups.map(function (backup) {
    return '<tr class="hover:bg-slate-50/80">' +
      '<td class="py-3 px-4 text-xs font-mono text-slate-700 break-all">' + sysEscape(backup.file || '-') + '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-500 whitespace-nowrap">' + sysEscape(sysFmtBytes(backup.size)) + '</td>' +
      '<td class="py-3 px-4 text-[11px] font-mono text-slate-400 whitespace-nowrap">' + sysEscape(sysFmtTime(backup.created_at)) + '</td>' +
      '</tr>';
  }).join('');
}

async function sysSystemFetchInfo() {
  const info = await sysApi('/api/system/info');
  sysState.system.info = info || {};
  sysRenderSystemInfo();
}

async function sysSystemFetchSettings() {
  const list = await sysApi('/api/settings');
  sysState.system.settings = sysAsArray(list);
  sysRenderSystemSettingsGroups(sysState.system.settings);
}

async function sysSystemFetchBackups() {
  const list = await sysApi('/api/system/backups');
  sysState.system.backups = sysAsArray(list);
  sysRenderSystemBackups();
}

async function sysSystemRefreshAll() {
  const st = sysState.system;
  if (st.loading) return;
  st.loading = true;
  try {
    const results = await Promise.allSettled([
      sysSystemFetchInfo(),
      sysSystemFetchSettings(),
      sysSystemFetchBackups()
    ]);
    sysReportResults('system', results);
    if (results[0].status === 'rejected' && !sysIsAuthError(results[0].reason)) {
      sysErrorBlock('sys-system-info-body', '系统信息加载失败', results[0].reason);
    }
    if (results[1].status === 'rejected' && !sysIsAuthError(results[1].reason)) {
      const container = document.getElementById('sys-settings-groups');
      if (container) {
        container.innerHTML = '<div class="bg-white rounded-xl border border-slate-200 shadow-sm">' +
          sysErrorBlockHtml('系统设置加载失败', results[1].reason) + '</div>';
      }
    }
    if (results[2].status === 'rejected' && !sysIsAuthError(results[2].reason)) {
      sysErrorTable('sys-system-backups-tbody', '备份列表加载失败', results[2].reason);
    }
    if (results.some(function (r) { return r.status === 'fulfilled'; })) {
      sysHideAuth('system');
    }
  } catch (e) {
    sysHandleError('system', e);
  } finally {
    st.loading = false;
  }
}

async function loadSysSystem() {
  try {
    const st = sysState.system;
    if (!st.ready || !document.getElementById('sys-system-root')) {
      sysRenderSystemSkeleton();
      st.ready = true;
    }
    await sysSystemRefreshAll();
  } catch (e) {
    sysHandleError('system', e);
  }
}

async function sysSystemSaveSettings() {
  const container = document.getElementById('sys-settings-groups');
  if (!container) return;
  const payload = {};
  container.querySelectorAll('[data-setting-key]').forEach(function (el) {
    const key = el.getAttribute('data-setting-key');
    const type = el.getAttribute('data-value-type');
    payload[key] = type === 'bool' ? (el.checked ? 'true' : 'false') : el.value;
  });
  if (!Object.keys(payload).length) {
    return sysToast('没有可保存的设置项', true);
  }
  const btn = document.getElementById('sys-system-save-btn');
  if (btn) {
    btn.disabled = true;
    btn.textContent = '保存中…';
    btn.classList.add('opacity-60');
  }
  try {
    const updated = await sysApi('/api/settings', sysJsonOptions('PUT', { settings: payload }));
    if (Array.isArray(updated)) {
      sysState.system.settings = updated;
      sysRenderSystemSettingsGroups(updated);
    } else {
      await sysSystemFetchSettings();
    }
    sysToast('系统设置已保存');
  } catch (e) {
    sysHandleError('system', e);
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = '保存设置';
      btn.classList.remove('opacity-60');
    }
  }
}

async function sysSystemBackup() {
  const btn = document.getElementById('sys-system-backup-btn');
  if (btn) {
    btn.disabled = true;
    btn.textContent = '备份中…';
    btn.classList.add('opacity-60');
  }
  try {
    const res = await sysApi('/api/system/backup', { method: 'POST' });
    const message = (res && res.message) || ('备份已创建' + (res && res.file ? '：' + res.file : ''));
    sysToast(message);
    await sysSystemFetchBackups();
  } catch (e) {
    sysHandleError('system', e);
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = '立即备份';
      btn.classList.remove('opacity-60');
    }
  }
}

async function sysSystemCleanup() {
  const days = parseInt(sysVal('sys-system-cleanup-days'), 10);
  if (!Number.isFinite(days) || days < 1) return sysToast('请输入大于 0 的保留天数', true);
  if (!window.confirm('确认清理 ' + days + ' 天前的操作日志、通知日志与发货记录？该操作不可恢复。')) return;
  try {
    const res = await sysApi('/api/system/cleanup', sysJsonOptions('POST', { days: days }));
    sysToast('已清理 操作日志 ' + sysNum(res && res.deleted_logs) +
      ' 条、通知日志 ' + sysNum(res && res.deleted_notifications) +
      ' 条、发货记录 ' + sysNum(res && res.deleted_deliveries) + ' 条');
  } catch (e) {
    sysHandleError('system', e);
  }
}

// ============================================================================
// 8. 管理员页（#view-admins）
// ============================================================================

function sysRoleBadge(role) {
  const tones = { super_admin: 'indigo', admin: 'emerald', viewer: 'slate' };
  return sysBadge(SYS_ROLE_LABELS[role] || role || '-', tones[role] || 'slate');
}

function sysAdminStatusBadge(status) {
  const tones = { active: 'emerald', disabled: 'slate' };
  return sysBadge(SYS_ADMIN_STATUS_LABELS[status] || status || '-', tones[status] || 'slate');
}

function sysRenderAdminsSkeleton() {
  const view = document.getElementById('view-admins');
  if (!view) return;
  view.innerHTML =
    '<div id="sys-admins-root" class="space-y-4">' +
    sysAuthBannerHtml('admins') +
    '<div id="sys-admins-me-card" class="bg-white rounded-xl border border-slate-200 shadow-sm">' +
    '<div data-sys-loading-block class="px-6 py-5">' +
    '<div class="text-xs text-slate-400">正在读取当前登录信息…</div>' +
    '</div>' +
    '</div>' +

    '<div class="grid grid-cols-2 lg:grid-cols-4 gap-4">' +
    sysStatCard('管理员总数', 'sys-admins-stat-total', 'GET /api/admins', 'text-slate-900') +
    sysStatCard('已启用', 'sys-admins-stat-active', 'status = active', 'text-emerald-600') +
    sysStatCard('超级管理员', 'sys-admins-stat-super', 'role = super_admin', 'text-indigo-600') +
    sysStatCard('登录失败', 'sys-admins-stat-failed-login', '最近 30 条登录日志', 'text-rose-600') +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">管理员列表</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">删除受后端保护：不能删除当前登录账号与最后一个 super_admin。</p>' +
    '</div>' +
    '<div class="flex gap-2">' +
    '<button onclick="sysAdminsRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新</button>' +
    '<button onclick="sysOpenAdminModal()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">+ 新建管理员</button>' +
    '</div>' +
    '</div>' +
    '<div class="overflow-x-auto">' +
    '<table class="w-full text-left">' +
    '<thead><tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">' +
    '<th class="py-3 px-4">用户名</th>' +
    '<th class="py-3 px-4">显示名</th>' +
    '<th class="py-3 px-4">角色</th>' +
    '<th class="py-3 px-4">状态</th>' +
    '<th class="py-3 px-4">最后登录</th>' +
    '<th class="py-3 px-4">创建时间</th>' +
    '<th class="py-3 px-4">操作</th>' +
    '</tr></thead>' +
    '<tbody id="sys-admins-tbody" data-sys-loading-body data-colspan="7" class="divide-y divide-slate-100 text-sm">' +
    '<tr><td colspan="7" class="py-10 px-4 text-center text-slate-400 text-xs">正在加载管理员列表…</td></tr>' +
    '</tbody>' +
    '</table>' +
    '</div>' +
    '</div>' +

    '<div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">' +
    '<div class="px-6 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">' +
    '<div>' +
    '<h3 class="text-sm font-bold text-slate-900">登录日志</h3>' +
    '<p class="text-[11px] text-slate-400 mt-0.5">最近 30 条管理员登录记录（GET /api/admins/login-logs?limit=30）。</p>' +
    '</div>' +
    '<button onclick="sysAdminsRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新日志</button>' +
    '</div>' +
    '<div class="overflow-x-auto">' +
    '<table class="w-full text-left">' +
    '<thead><tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">' +
    '<th class="py-3 px-4">用户名</th>' +
    '<th class="py-3 px-4">结果</th>' +
    '<th class="py-3 px-4">IP</th>' +
    '<th class="py-3 px-4">User-Agent</th>' +
    '<th class="py-3 px-4">消息</th>' +
    '<th class="py-3 px-4">时间</th>' +
    '</tr></thead>' +
    '<tbody id="sys-admins-login-tbody" data-sys-loading-body data-colspan="6" class="divide-y divide-slate-100 text-sm">' +
    '<tr><td colspan="6" class="py-10 px-4 text-center text-slate-400 text-xs">正在加载登录日志…</td></tr>' +
    '</tbody>' +
    '</table>' +
    '</div>' +
    '</div>' +
    '</div>';
}

function sysRenderAdminMe() {
  const card = document.getElementById('sys-admins-me-card');
  if (!card) return;
  const st = sysState.admins;
  const me = st.me;
  if (!st.authenticated || !me) {
    card.innerHTML =
      '<div class="px-6 py-5 flex flex-wrap items-center justify-between gap-4">' +
      '<div class="flex items-center gap-3">' +
      '<div class="w-10 h-10 rounded-xl bg-amber-50 text-amber-600 flex items-center justify-center text-lg">🔒</div>' +
      '<div>' +
      '<div class="text-sm font-bold text-slate-900">' + SYS_AUTH_TEXT + '</div>' +
      '<div class="text-xs text-slate-500 mt-0.5">未检测到有效登录态，登录后可管理管理员账号与查看登录日志。</div>' +
      '</div>' +
      '</div>' +
      '<div class="flex gap-2">' +
      '<button onclick="openLoginModal()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">登录</button>' +
      '<button onclick="sysAdminsRefreshAll()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">刷新</button>' +
      '</div>' +
      '</div>';
    return;
  }
  const initial = (me.username || 'A').slice(0, 1).toUpperCase();
  card.innerHTML =
    '<div class="px-6 py-5 flex flex-wrap items-start justify-between gap-4">' +
    '<div class="flex items-start gap-3">' +
    '<div class="w-10 h-10 rounded-xl bg-indigo-50 text-indigo-600 flex items-center justify-center font-bold">' + sysEscape(initial) + '</div>' +
    '<div>' +
    '<div class="flex flex-wrap items-center gap-2">' +
    '<span class="text-sm font-bold text-slate-900">' + sysEscape(me.display_name || me.username || '管理员') + '</span>' +
    (me.role ? sysRoleBadge(me.role) : sysBadge('未知角色', 'slate')) +
    sysBadge('已登录', 'emerald') +
    '</div>' +
    '<div class="text-[11px] text-slate-500 mt-1.5 font-mono">用户名: ' + sysEscape(me.username || '-') +
    ' · admin_id: ' + sysEscape(me.admin_id === null || me.admin_id === undefined ? '-' : me.admin_id) +
    ' · 会话到期: ' + sysEscape(sysFmtTime(me.expires_at)) + '</div>' +
    '</div>' +
    '</div>' +
    '<div class="flex gap-2">' +
    '<button onclick="sysOpenChangePasswordModal()" class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-medium">修改我的密码</button>' +
    '<button onclick="sysAdminsRefreshAll()" class="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">刷新</button>' +
    '</div>' +
    '</div>';
}

function sysRenderAdminStats() {
  const admins = sysState.admins.admins || [];
  const logs = sysState.admins.logs || [];
  sysSetText('sys-admins-stat-total', admins.length);
  sysSetText('sys-admins-stat-active', admins.filter(function (admin) { return admin.status === 'active'; }).length);
  sysSetText('sys-admins-stat-super', admins.filter(function (admin) { return admin.role === 'super_admin'; }).length);
  sysSetText('sys-admins-stat-failed-login', logs.filter(function (log) { return !log.success; }).length);
}

function sysRenderAdmins() {
  const tbody = document.getElementById('sys-admins-tbody');
  if (!tbody) return;
  const admins = sysState.admins.admins || [];
  if (!admins.length) {
    tbody.innerHTML = sysTableEmpty(7, '暂无管理员账号', '点击「+ 新建管理员」创建第一个管理员账号。');
    return;
  }
  tbody.innerHTML = admins.map(function (admin) {
    const id = sysNum(admin.id);
    const loginAt = admin.last_login_at ? sysFmtTime(admin.last_login_at) : '-';
    return '<tr class="hover:bg-slate-50/80 align-top">' +
      '<td class="py-3 px-4 text-xs font-mono font-semibold text-slate-900">' + sysEscape(admin.username || '-') + '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-700">' + sysEscape(admin.display_name || '-') + '</td>' +
      '<td class="py-3 px-4">' + sysRoleBadge(admin.role) + '</td>' +
      '<td class="py-3 px-4">' + sysAdminStatusBadge(admin.status) + '</td>' +
      '<td class="py-3 px-4 text-[11px] text-slate-500 font-mono">' +
      '<div>' + sysEscape(loginAt) + '</div>' +
      '<div class="text-slate-400">' + sysEscape(admin.last_login_ip || '-') + '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-[11px] text-slate-400 font-mono whitespace-nowrap">' + sysEscape(sysFmtTime(admin.created_at)) + '</td>' +
      '<td class="py-3 px-4">' +
      '<div class="flex flex-wrap gap-1.5">' +
      '<button onclick="sysOpenAdminModal(' + id + ')" class="px-2.5 py-1 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-[11px] font-medium">编辑</button>' +
      '<button onclick="sysOpenResetPasswordModal(' + id + ')" class="px-2.5 py-1 rounded-lg bg-indigo-50 hover:bg-indigo-100 text-indigo-700 text-[11px] font-medium">重置密码</button>' +
      '<button onclick="sysDeleteAdmin(' + id + ')" class="px-2.5 py-1 rounded-lg bg-rose-50 hover:bg-rose-100 text-rose-600 text-[11px] font-medium">删除</button>' +
      '</div>' +
      '</td>' +
      '</tr>';
  }).join('');
}

function sysRenderAdminLoginLogs() {
  const tbody = document.getElementById('sys-admins-login-tbody');
  if (!tbody) return;
  const logs = sysState.admins.logs || [];
  if (!logs.length) {
    tbody.innerHTML = sysTableEmpty(6, '暂无登录日志', '管理员登录成功或失败都会写入 admin_login_logs。');
    return;
  }
  tbody.innerHTML = logs.map(function (log) {
    return '<tr class="hover:bg-slate-50/80 align-top">' +
      '<td class="py-3 px-4 text-xs font-mono text-slate-800">' + sysEscape(log.username || '-') + '</td>' +
      '<td class="py-3 px-4">' + (log.success ? sysBadge('成功', 'emerald') : sysBadge('失败', 'rose')) + '</td>' +
      '<td class="py-3 px-4 text-[11px] font-mono text-slate-500 whitespace-nowrap">' + sysEscape(log.ip || '-') + '</td>' +
      '<td class="py-3 px-4 text-[11px] text-slate-500 max-w-[240px]">' +
      '<div class="truncate" title="' + sysEscape(log.user_agent || '') + '">' + sysEscape(sysTruncate(log.user_agent || '-', 40)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-xs text-slate-600 max-w-[200px]">' +
      '<div class="truncate" title="' + sysEscape(log.message || '') + '">' + sysEscape(sysTruncate(log.message || '-', 30)) + '</div>' +
      '</td>' +
      '<td class="py-3 px-4 text-[11px] font-mono text-slate-400 whitespace-nowrap">' + sysEscape(sysFmtTime(log.created_at)) + '</td>' +
      '</tr>';
  }).join('');
}

async function sysAdminFetchMe() {
  const st = sysState.admins;
  try {
    const me = await sysApi('/api/auth/me');
    st.me = me && me.authenticated ? me : null;
  } catch (e) {
    st.me = null;
    if (e && e.status !== 403 && e.status !== 404 && !sysIsAuthError(e)) {
      sysToast(e.message || '读取当前登录信息失败', true);
    }
  }
  st.authenticated = !!st.me;
  sysRenderAdminMe();
}

async function sysAdminFetchAdmins() {
  const list = await sysApi('/api/admins');
  sysState.admins.admins = sysAsArray(list);
  sysRenderAdmins();
  sysRenderAdminStats();
}

async function sysAdminFetchLoginLogs() {
  const list = await sysApi('/api/admins/login-logs?limit=30');
  sysState.admins.logs = sysAsArray(list);
  sysRenderAdminLoginLogs();
  sysRenderAdminStats();
}

async function sysAdminsRefreshAll() {
  const st = sysState.admins;
  if (st.loading) return;
  st.loading = true;
  try {
    await sysAdminFetchMe();
    const results = await Promise.allSettled([
      sysAdminFetchAdmins(),
      sysAdminFetchLoginLogs()
    ]);
    sysReportResults('admins', results);
    if (results[0].status === 'rejected' && !sysIsAuthError(results[0].reason)) {
      sysErrorTable('sys-admins-tbody', '管理员列表加载失败', results[0].reason);
    }
    if (results[1].status === 'rejected' && !sysIsAuthError(results[1].reason)) {
      sysErrorTable('sys-admins-login-tbody', '登录日志加载失败', results[1].reason);
    }
    if (results.some(function (r) { return r.status === 'fulfilled'; })) {
      sysHideAuth('admins');
    }
  } catch (e) {
    sysHandleError('admins', e);
  } finally {
    st.loading = false;
  }
}

async function loadSysAdmins() {
  try {
    const st = sysState.admins;
    if (!st.ready || !document.getElementById('sys-admins-root')) {
      sysRenderAdminsSkeleton();
      st.ready = true;
    }
    await sysAdminsRefreshAll();
  } catch (e) {
    sysHandleError('admins', e);
  }
}

function sysOpenAdminModal(id) {
  const admin = id ? (sysState.admins.admins || []).find(function (item) { return sysNum(item.id) === sysNum(id); }) : null;
  const role = admin ? (admin.role || 'admin') : 'admin';
  const status = admin ? (admin.status || 'active') : 'active';
  sysEnsureModal('sys-admin-modal',
    sysModalHeader(admin ? '编辑管理员' : '新建管理员', 'sys-admin-modal') +
    '<input type="hidden" id="sys-admin-id" value="' + (admin ? sysNum(admin.id) : '') + '" />' +
    '<div class="space-y-3 text-xs">' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">用户名 *</label>' +
    '<input id="sys-admin-username" type="text" value="' + sysEscape(admin ? (admin.username || '') : '') + '"' +
    (admin ? ' disabled' : '') +
    ' placeholder="例如：ops" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono disabled:bg-slate-100" />' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">' + (admin ? '重置密码（留空不修改）' : '密码 *') + '</label>' +
    '<input id="sys-admin-password" type="password" autocomplete="new-password" placeholder="' + (admin ? '留空表示不修改密码' : '至少由后端密码策略校验') + '" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono" />' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">显示名</label>' +
    '<input id="sys-admin-display-name" type="text" value="' + sysEscape(admin ? (admin.display_name || '') : '') + '" placeholder="例如：运营" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm" />' +
    '</div>' +
    '<div class="grid grid-cols-2 gap-3">' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">角色</label>' +
    '<select id="sys-admin-role" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm">' +
    sysSelectOptions(SYS_ROLE_LABELS, role) +
    '</select>' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">状态</label>' +
    '<select id="sys-admin-status" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm">' +
    sysSelectOptions(SYS_ADMIN_STATUS_LABELS, status) +
    '</select>' +
    '</div>' +
    '</div>' +
    '<div class="text-[11px] text-slate-400">角色：super_admin（超级管理员）/ admin（管理员）/ viewer（只读）。编辑时用户名不可修改，密码留空表示不修改。</div>' +
    '</div>' +
    '<div class="flex justify-end gap-2 pt-2">' +
    sysModalCancelButton('sys-admin-modal') +
    '<button onclick="sysSubmitAdmin()" class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">保存管理员</button>' +
    '</div>'
  );
  sysOpenModal('sys-admin-modal');
}

async function sysSubmitAdmin() {
  const id = sysVal('sys-admin-id');
  const username = sysVal('sys-admin-username');
  const password = sysRawVal('sys-admin-password');
  const displayName = sysVal('sys-admin-display-name');
  const role = sysVal('sys-admin-role') || 'admin';
  const status = sysVal('sys-admin-status') || 'active';
  if (!id && !username) return sysToast('请填写用户名', true);
  if (!id && !password) return sysToast('请填写密码', true);
  const payload = id
    ? { display_name: displayName, role: role, status: status, password: password || null }
    : { username: username, password: password, display_name: displayName, role: role, status: status };
  try {
    if (id) {
      await sysApi('/api/admins/' + id, sysJsonOptions('PUT', payload));
    } else {
      await sysApi('/api/admins', sysJsonOptions('POST', payload));
    }
    sysCloseModal('sys-admin-modal');
    sysToast(id ? '管理员信息已更新' : '管理员已创建');
    await sysAdminFetchAdmins();
  } catch (e) {
    if (sysIsAuthError(e)) sysCloseModal('sys-admin-modal');
    sysHandleError('admins', e);
  }
}

function sysOpenResetPasswordModal(id) {
  const admin = (sysState.admins.admins || []).find(function (item) { return sysNum(item.id) === sysNum(id); });
  if (!admin) return sysToast('未找到该管理员', true);
  sysEnsureModal('sys-admin-reset-modal',
    sysModalHeader('重置密码 · ' + admin.username, 'sys-admin-reset-modal') +
    '<input type="hidden" id="sys-reset-admin-id" value="' + sysNum(admin.id) + '" />' +
    '<div class="space-y-3 text-xs">' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">新密码 *</label>' +
    '<input id="sys-reset-password" type="password" autocomplete="new-password" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono" />' +
    '</div>' +
    '<div class="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-[11px] text-amber-700">将直接覆盖管理员「' + sysEscape(admin.username) + '」的登录密码，请通知对方。</div>' +
    '</div>' +
    '<div class="flex justify-end gap-2 pt-2">' +
    sysModalCancelButton('sys-admin-reset-modal') +
    '<button onclick="sysSubmitResetPassword()" class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">重置密码</button>' +
    '</div>'
  );
  sysOpenModal('sys-admin-reset-modal');
}

async function sysSubmitResetPassword() {
  const id = sysVal('sys-reset-admin-id');
  const password = sysRawVal('sys-reset-password');
  if (!id) return sysToast('缺少管理员 ID', true);
  if (!password) return sysToast('请输入新密码', true);
  try {
    await sysApi('/api/admins/' + id, sysJsonOptions('PUT', { password: password }));
    sysCloseModal('sys-admin-reset-modal');
    sysToast('密码已重置');
  } catch (e) {
    if (sysIsAuthError(e)) sysCloseModal('sys-admin-reset-modal');
    sysHandleError('admins', e);
  }
}

async function sysDeleteAdmin(id) {
  const admin = (sysState.admins.admins || []).find(function (item) { return sysNum(item.id) === sysNum(id); });
  const name = admin ? admin.username : String(id);
  if (!window.confirm('确认删除管理员「' + name + '」？删除后该账号无法登录，后端会拒绝删除当前账号或最后一个 super_admin。')) return;
  try {
    await sysApi('/api/admins/' + id, { method: 'DELETE' });
    sysToast('管理员已删除');
    await sysAdminFetchAdmins();
  } catch (e) {
    if (sysIsAuthError(e)) {
      sysShowAuth('admins');
      return;
    }
    sysToast(e.message || '删除失败（后端拒绝删除当前账号或最后一个 super_admin）', true);
  }
}

function sysOpenChangePasswordModal() {
  sysEnsureModal('sys-admin-password-modal',
    sysModalHeader('修改我的密码', 'sys-admin-password-modal') +
    '<div class="space-y-3 text-xs">' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">原密码 *</label>' +
    '<input id="sys-pwd-old" type="password" autocomplete="current-password" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono" />' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">新密码 *</label>' +
    '<input id="sys-pwd-new" type="password" autocomplete="new-password" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono" />' +
    '</div>' +
    '<div>' +
    '<label class="block text-slate-700 font-semibold mb-1">确认新密码 *</label>' +
    '<input id="sys-pwd-confirm" type="password" autocomplete="new-password" class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono" />' +
    '</div>' +
    '<div class="text-[11px] text-slate-400">提交到 POST /api/auth/change-password，密码长度等规则由后端设置校验。</div>' +
    '</div>' +
    '<div class="flex justify-end gap-2 pt-2">' +
    sysModalCancelButton('sys-admin-password-modal') +
    '<button onclick="sysSubmitChangePassword()" class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">更新密码</button>' +
    '</div>'
  );
  sysOpenModal('sys-admin-password-modal');
}

async function sysSubmitChangePassword() {
  const oldPassword = sysRawVal('sys-pwd-old');
  const newPassword = sysRawVal('sys-pwd-new');
  const confirmPassword = sysRawVal('sys-pwd-confirm');
  if (!oldPassword || !newPassword) return sysToast('请填写原密码与新密码', true);
  if (newPassword !== confirmPassword) return sysToast('两次输入的新密码不一致', true);
  if (newPassword === oldPassword) return sysToast('新密码不能与原密码相同', true);
  try {
    const res = await sysApi('/api/auth/change-password', sysJsonOptions('POST', {
      old_password: oldPassword,
      new_password: newPassword
    }));
    sysCloseModal('sys-admin-password-modal');
    sysToast((res && res.message) || '密码已更新');
  } catch (e) {
    if (sysIsAuthError(e)) sysCloseModal('sys-admin-password-modal');
    sysHandleError('admins', e);
  }
}

// ============================================================================
// 9. 页面加载器注册（必须在 index.html 内联脚本之前完成，只注册不调用）
// ============================================================================

SYS_PAGE_LOADERS.scheduler = loadSysScheduler;
SYS_PAGE_LOADERS.logs = loadSysLogs;
SYS_PAGE_LOADERS.notifications = loadSysNotifications;
SYS_PAGE_LOADERS.system = loadSysSystem;
SYS_PAGE_LOADERS.admins = loadSysAdmins;

window.pageLoaders = window.pageLoaders || {};
window.pageLoaders['scheduler'] = loadSysScheduler;
window.pageLoaders['logs'] = loadSysLogs;
window.pageLoaders['notifications'] = loadSysNotifications;
window.pageLoaders['system'] = loadSysSystem;
window.pageLoaders['admins'] = loadSysAdmins;
