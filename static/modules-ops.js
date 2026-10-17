/**
 * AutoDeliver 管理后台 - 运营页面模块
 * ---------------------------------------------------------------------------
 * 负责：在线消息（messages）、自动发货（autodeliver）、发货记录（deliveries）。
 *
 * 加载约定：
 *   - 本文件在 static/index.html 的内联脚本之前加载，顶层只做函数定义与注册；
 *   - 进入页面时由 switchTab() 调用 window.pageLoaders[tab]()；
 *   - 所有动态 HTML 必须经过 opsEscape() 转义；
 *   - 所有接口错误在页面内消化，401 统一展示登录引导，不抛出未捕获异常。
 */

'use strict';

/* =========================================================================
 * 一、通用工具
 * ========================================================================= */

/** HTML 转义（用于文本与属性值）。 */
function opsEscape(value) {
  if (value === null || value === undefined) return '';
  return String(value)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** 解析闲鱼昵称（兼容全局 decodeNick）。 */
function opsDecode(value) {
  if (value === null || value === undefined || value === '') return '';
  try {
    if (typeof window.decodeNick === 'function') return window.decodeNick(String(value));
    return decodeURIComponent(String(value));
  } catch (_) {
    return String(value);
  }
}

/** 统一 toast，避免脚本脱离 index.html 时抛错。 */
function opsToast(message, isError) {
  const text = message === null || message === undefined || message === '' ? '操作完成' : String(message);
  if (typeof window.showToast === 'function') {
    try {
      window.showToast(text, !!isError);
      return;
    } catch (_) {
      /* 页面没有 #toast 时退回 console */
    }
  }
  if (isError) console.error('[AutoDeliver]', text);
  else console.log('[AutoDeliver]', text);
}

/** 是否是登录态错误（兼容 apiFetch 只有 message、没有 status 的情况）。 */
function opsIsAuthError(error) {
  if (!error) return false;
  if (error.isAuth || error.status === 401 || error.code === 'UNAUTHORIZED') return true;
  const message = String(error.message || error);
  return /(401|UNAUTHORIZED|未登录|请先登录|会话已过期|登录已过期|登录失效|会话失效)/i.test(message);
}

/**
 * 统一请求入口：优先复用全局 apiFetch（自动附带 Bearer token），
 * 并把它抛出的登录错误标准化为 isAuth 错误，便于页面统一展示登录卡片。
 */
async function opsRequest(url, options) {
  if (typeof window.apiFetch === 'function') {
    try {
      return await window.apiFetch(url, options || {});
    } catch (error) {
      if (opsIsAuthError(error)) {
        const authError = new Error('请先登录管理员账号');
        authError.isAuth = true;
        authError.status = 401;
        authError.cause = error;
        throw authError;
      }
      throw error;
    }
  }
  return opsRawRequest(url, options);
}

/** apiFetch 不可用时的兜底请求实现（仍会附带 Authorization）。 */
async function opsRawRequest(url, options) {
  const opts = options || {};
  const headers = new Headers(opts.headers || {});
  let token = '';
  try {
    token = localStorage.getItem('adm_token') || '';
  } catch (_) {
    token = '';
  }
  if (token && !headers.has('Authorization')) headers.set('Authorization', `Bearer ${token}`);

  const response = await fetch(url, Object.assign({}, opts, { headers }));
  if (response.status === 401) {
    const authError = new Error('请先登录管理员账号');
    authError.isAuth = true;
    authError.status = 401;
    throw authError;
  }

  const text = await response.text();
  let data = {};
  if (text) {
    try {
      data = JSON.parse(text);
    } catch (_) {
      throw new Error(`接口 ${url} 返回了非 JSON 响应（HTTP ${response.status}）`);
    }
  }
  if (!response.ok) {
    const error = new Error((data && data.message) || `请求失败（HTTP ${response.status}）`);
    error.status = response.status;
    error.code = data && data.code;
    throw error;
  }
  return data;
}

/** 显示/隐藏某页面的登录引导。prefix: msg | ad | dlv */
function opsSetAuthState(prefix, showAuth) {
  const notice = document.getElementById(`ops-${prefix}-auth`);
  const body = document.getElementById(`ops-${prefix}-body`);
  if (notice) notice.classList.toggle('hidden', !showAuth);
  if (body) body.classList.toggle('hidden', !!showAuth);
}

/** 登录引导卡片。 */
function opsAuthNoticeHtml(prefix, description) {
  return `
    <div id="ops-${prefix}-auth" class="hidden bg-white rounded-xl border border-rose-200 shadow-sm p-10 text-center space-y-3">
      <div class="w-12 h-12 mx-auto rounded-full bg-rose-50 flex items-center justify-center text-2xl">🔒</div>
      <div class="text-sm font-bold text-slate-900">请先登录管理员账号</div>
      <p class="text-xs text-slate-500 leading-relaxed">${opsEscape(description || '登录后可查看该页面数据。')}</p>
      <div class="flex flex-wrap items-center justify-center gap-2 pt-1">
        <button type="button" onclick="openLoginModal()"
          class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold shadow-xs">
          登录管理后台
        </button>
        <button type="button" onclick="opsReloadAuthPage('${prefix}')"
          class="px-4 py-2 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-semibold border border-slate-300">
          已登录，重新加载
        </button>
      </div>
    </div>
  `;
}

/** 登录完成后，从登录引导卡片按当前模块重新加载页面。 */
function opsReloadAuthPage(prefix) {
  if (prefix === 'msg') return loadOpsMessages();
  if (prefix === 'ad') return loadOpsAutoDeliver();
  if (prefix === 'dlv') return loadOpsDeliveries();
  return undefined;
}

/** 页面级错误处理：返回 true 表示已按登录态问题处理。 */
function opsHandleAuthError(prefix, error) {
  if (!opsIsAuthError(error)) return false;
  try {
    localStorage.removeItem('adm_token');
  } catch (_) {
    /* ignore */
  }
  opsSetAuthState(prefix, true);
  return true;
}

/** 只用于给 onclick 传递数值 ID，避免任何字符串注入。 */
function opsSafeInt(value) {
  const number = Number(value);
  return Number.isFinite(number) ? Math.trunc(number) : 0;
}

function opsSetText(id, value) {
  const el = document.getElementById(id);
  if (el) el.textContent = value === null || value === undefined ? '' : String(value);
}

/** 闲鱼宝贝详情页链接。 */
function opsItemUrl(itemId) {
  const clean = String(itemId || '').trim();
  if (!clean) return '';
  return `https://www.goofish.com/item?id=${encodeURIComponent(clean)}`;
}

function opsSetValue(id, value) {
  const el = document.getElementById(id);
  if (el) el.value = value === null || value === undefined ? '' : String(value);
}

function opsSetChecked(id, checked) {
  const el = document.getElementById(id);
  if (el) el.checked = !!checked;
}

function opsSetHidden(id, hidden) {
  const el = document.getElementById(id);
  if (el) el.classList.toggle('hidden', !!hidden);
}

/** 数量展示。 */
function opsCount(value) {
  const number = Number(value);
  return Number.isFinite(number) ? String(number) : '-';
}

/** 成功率展示。 */
function opsPercent(value) {
  const number = Number(value);
  return Number.isFinite(number) ? `${number.toFixed(1)}%` : '-';
}

/** 金额：分 → 元。 */
function opsYuan(fen) {
  const number = Number(fen);
  return Number.isFinite(number) ? (number / 100).toFixed(2) : '-';
}

/** 时间：去掉 T，格式化到秒。 */
function opsRawTime(value) {
  if (value === null || value === undefined || value === '') return '';
  return String(value).replace('T', ' ').replace(/\.\d+$/, '').replace(/Z$/, '');
}

function opsFmtTime(value) {
  const text = opsRawTime(value);
  return text ? text.slice(0, 19) : '-';
}

/** 会话列表用短时间：今天显示“今天 HH:mm”，否则 MM-DD HH:mm。 */
function opsFmtShortTime(value) {
  const text = opsRawTime(value);
  if (!text) return '';
  const match = text.match(/^(\d{4})-(\d{2})-(\d{2})[ ](\d{2}:\d{2})/);
  if (!match) return text;
  const now = new Date();
  const sameDay =
    Number(match[1]) === now.getFullYear() &&
    Number(match[2]) === now.getMonth() + 1 &&
    Number(match[3]) === now.getDate();
  if (sameDay) return `今天 ${match[4]}`;
  if (Number(match[1]) !== now.getFullYear()) return `${match[1]}-${match[2]}-${match[3]}`;
  return `${match[2]}-${match[3]} ${match[4]}`;
}

/** 耗时展示。 */
function opsFormatDuration(ms) {
  const number = Number(ms);
  if (!Number.isFinite(number) || number < 0) return '-';
  if (number < 1000) return `${Math.round(number)} ms`;
  if (number < 60000) return `${(number / 1000).toFixed(2)} s`;
  return `${Math.floor(number / 60000)} 分 ${Math.round((number % 60000) / 1000)} 秒`;
}

/** 读取 index.html 的全局当前闲鱼账号（let 声明，不能通过 window 访问）。 */
function opsCurrentAccountId() {
  try {
    if (typeof currentAccountId !== 'undefined' && currentAccountId) {
      const number = Number(currentAccountId);
      return Number.isFinite(number) ? number : null;
    }
  } catch (_) {
    /* ignore */
  }
  return null;
}

/** 获取闲鱼账号列表；接口异常时退回 index.html 的全局缓存。 */
async function opsLoadXianyuAccounts() {
  try {
    const data = await opsRequest('/api/xianyu/accounts');
    if (Array.isArray(data)) return data;
    if (data && Array.isArray(data.items)) return data.items;
    return [];
  } catch (error) {
    if (opsIsAuthError(error)) throw error;
    try {
      if (Array.isArray(xianyuAccounts)) return xianyuAccounts.slice();
    } catch (_) {
      /* ignore */
    }
    return [];
  }
}

function opsAccountName(account, fallbackId) {
  if (account) {
    return opsDecode(account.seller_nick) || account.seller_id || `账号 #${opsSafeInt(account.id)}`;
  }
  return fallbackId ? `账号 #${opsSafeInt(fallbackId)}` : '未知账号';
}

/**
 * 渲染账号筛选下拉框，并尽量保留当前选择。
 * 返回最终选中的 value（账号被删除时自动回落为 ''）。
 */
function opsFillAccountSelect(selectId, accounts, selected, allLabel) {
  const select = document.getElementById(selectId);
  if (!select) return selected === null || selected === undefined ? '' : String(selected);
  const list = Array.isArray(accounts) ? accounts : [];
  const options = [`<option value="">${opsEscape(allLabel || '全部账号')}</option>`];
  list.forEach((account) => {
    const id = opsSafeInt(account.id);
    const label = opsAccountName(account, id);
    options.push(`<option value="${id}">#${id} ${opsEscape(label)}</option>`);
  });
  select.innerHTML = options.join('');
  const selectedValue = selected === null || selected === undefined ? '' : String(selected);
  const exists = selectedValue === '' || list.some((account) => String(account.id) === selectedValue);
  select.value = exists ? selectedValue : '';
  return select.value;
}

/* =========================================================================
 * 二、在线消息（#view-messages）
 * ========================================================================= */

var opsMessagesState = {
  rendered: false,
  accounts: [],
  conversations: [],
  stats: null,
  accountId: '',
  status: '',
  search: '',
  activeConversationId: null,
  activeConversation: null,
  listSeq: 0,
  threadSeq: 0,
  timerStarted: false,
  sending: false,
  syncing: false
};

function opsMessagesEnsureSkeleton() {
  const root = document.getElementById('view-messages');
  if (!root) return false;
  if (opsMessagesState.rendered && root.querySelector('#ops-msg-body')) return true;

  root.innerHTML = `
    <div class="space-y-4">
      ${opsAuthNoticeHtml('msg', '登录后可同步闲鱼会话、查看聊天记录并直接回复买家。')}

      <div id="ops-msg-body" class="space-y-4">
        <div class="grid grid-cols-2 lg:grid-cols-4 gap-3 sm:gap-4">
          <div class="bg-white rounded-xl p-4 border border-slate-200 shadow-sm">
            <div class="text-xs font-medium text-slate-500">💬 会话数</div>
            <div id="ops-msg-stat-conversations" class="text-2xl font-bold text-slate-900 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">全部私信会话</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-rose-200 shadow-sm">
            <div class="text-xs font-medium text-rose-600">🔴 未读数</div>
            <div id="ops-msg-stat-unread" class="text-2xl font-bold text-rose-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">等待回复的买家消息</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-slate-200 shadow-sm">
            <div class="text-xs font-medium text-indigo-600">📨 今日消息</div>
            <div id="ops-msg-stat-today" class="text-2xl font-bold text-indigo-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">今天收发的消息总数</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-emerald-200 shadow-sm">
            <div class="text-xs font-medium text-emerald-600">🟢 进行中会话</div>
            <div id="ops-msg-stat-open" class="text-2xl font-bold text-emerald-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">状态为 open 的会话</div>
          </div>
        </div>

        <div class="bg-white rounded-xl border border-slate-200 shadow-sm p-4">
          <div class="flex flex-wrap items-end gap-3">
            <div>
              <label class="block text-[11px] font-semibold text-slate-500 mb-1">闲鱼账号</label>
              <select id="ops-msg-account" onchange="opsMessagesAccountChanged(this.value)"
                class="min-w-[190px] rounded-lg border border-slate-300 bg-white px-3 py-2 text-xs font-semibold text-slate-700 focus:outline-none focus:border-indigo-500">
                <option value="">全部账号</option>
              </select>
            </div>
            <div>
              <label class="block text-[11px] font-semibold text-slate-500 mb-1">会话状态</label>
              <select id="ops-msg-status" onchange="opsMessagesStatusChanged(this.value)"
                class="rounded-lg border border-slate-300 bg-white px-3 py-2 text-xs font-semibold text-slate-700 focus:outline-none focus:border-indigo-500">
                <option value="">全部状态</option>
                <option value="open">进行中 (open)</option>
                <option value="closed">已关闭 (closed)</option>
              </select>
            </div>
            <div class="flex-1 min-w-[220px]">
              <label class="block text-[11px] font-semibold text-slate-500 mb-1">搜索</label>
              <input id="ops-msg-search" type="text" placeholder="买家昵称 / 会话 / 最后一条消息"
                onkeydown="opsMessagesSearchKeydown(event)"
                class="w-full rounded-lg border border-slate-300 px-3 py-2 text-xs" />
            </div>
            <div class="flex flex-wrap items-center gap-2">
              <button type="button" onclick="opsMessagesApplyFilters()"
                class="px-3.5 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold shadow-xs">
                🔍 筛选
              </button>
              <button type="button" id="ops-msg-sync-btn" onclick="opsMessagesSync()"
                class="px-3.5 py-2 rounded-lg bg-slate-900 hover:bg-slate-800 text-white text-xs font-semibold shadow-xs">
                🔄 同步消息
              </button>
              <button type="button" onclick="opsMessagesRefresh()"
                class="px-3.5 py-2 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-semibold border border-slate-300 shadow-xs">
                刷新
              </button>
            </div>
          </div>
          <p class="mt-2 text-[11px] text-slate-400">每 20 秒在本页可见时自动刷新会话列表；「同步消息」会从闲鱼 IM 拉取最新会话与消息。</p>
        </div>

        <div class="grid grid-cols-1 lg:grid-cols-12 gap-4 items-stretch">
          <div class="lg:col-span-4 bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden flex flex-col h-[640px] max-h-[75vh] min-h-[480px]">
            <div class="px-4 py-3 border-b border-slate-200 flex items-center justify-between shrink-0">
              <h2 class="text-sm font-bold text-slate-900">会话列表</h2>
              <span id="ops-msg-list-count" class="text-[11px] text-slate-400">0 个会话</span>
            </div>
            <div id="ops-msg-list" class="flex-1 min-h-0 overflow-y-auto divide-y divide-slate-100">
              <div class="px-4 py-12 text-center text-xs text-slate-400">正在加载会话...</div>
            </div>
          </div>

          <div class="lg:col-span-8 bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden flex flex-col h-[640px] max-h-[75vh] min-h-[480px]">
            <div class="px-5 py-3 border-b border-slate-200 flex items-center justify-between gap-3 shrink-0">
              <div class="min-w-0">
                <div id="ops-msg-active-title" class="text-sm font-bold text-slate-900 truncate">请选择左侧会话</div>
                <div id="ops-msg-active-meta" class="text-[11px] text-slate-500 truncate">点击会话查看聊天记录并回复买家</div>
              </div>
              <button type="button" id="ops-msg-delete" onclick="opsDeleteConversation()"
                class="hidden shrink-0 px-2.5 py-1.5 rounded-lg bg-rose-50 hover:bg-rose-100 text-rose-600 text-xs font-medium">
                删除会话
              </button>
            </div>

            <div id="ops-msg-thread" class="flex-1 min-h-0 overflow-y-auto p-4 space-y-3 bg-slate-50/60">
              <div class="h-full flex flex-col items-center justify-center text-center text-slate-400">
                <div class="text-3xl mb-2">💬</div>
                <div class="text-xs">点击左侧会话查看聊天记录</div>
              </div>
            </div>

            <div id="ops-msg-composer" class="hidden shrink-0 border-t border-slate-200 bg-white p-3 space-y-2">
              <textarea id="ops-msg-input" rows="2" placeholder="输入回复内容，Enter 发送，Shift+Enter 换行"
                onkeydown="opsMessagesInputKeydown(event)"
                class="w-full rounded-lg border border-slate-300 px-3 py-2 text-xs resize-none focus:outline-none focus:border-indigo-500"></textarea>
              <div class="flex items-center justify-between gap-3">
                <span id="ops-msg-send-hint" class="text-[11px] text-slate-400">消息将通过闲鱼 IM 发送给买家</span>
                <button type="button" id="ops-msg-send-btn" onclick="opsMessagesSend()"
                  class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold shadow-xs">
                  发送
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  `;

  opsMessagesState.rendered = true;
  opsSetValue('ops-msg-status', opsMessagesState.status);
  opsSetValue('ops-msg-search', opsMessagesState.search);
  return true;
}

function opsMessagesRenderStats(stats) {
  const data = stats || {};
  opsSetText('ops-msg-stat-conversations', opsCount(data.conversations));
  opsSetText('ops-msg-stat-unread', opsCount(data.unread));
  opsSetText('ops-msg-stat-today', opsCount(data.today_messages));
  opsSetText('ops-msg-stat-open', opsCount(data.open_conversations));

  const badge = document.getElementById('badge-messages');
  if (badge) {
    badge.textContent = opsCount(data.conversations);
    badge.className = Number(data.unread) > 0
      ? 'text-xs px-2 py-0.5 rounded-full bg-rose-500 text-white'
      : 'text-xs px-2 py-0.5 rounded-full bg-slate-800 text-slate-400';
  }
}

function opsMessagesRenderList() {
  const listEl = document.getElementById('ops-msg-list');
  if (!listEl) return;
  const state = opsMessagesState;
  const conversations = Array.isArray(state.conversations) ? state.conversations : [];

  opsSetText('ops-msg-list-count', `${conversations.length} 个会话`);

  if (conversations.length === 0) {
    const filtered = !!(state.search || state.status || state.accountId);
    listEl.innerHTML = `
      <div class="px-5 py-12 text-center text-slate-400">
        <div class="text-3xl mb-2">${filtered ? '🔍' : '📭'}</div>
        <div class="text-xs font-medium">${filtered ? '没有符合筛选条件的会话' : '暂无会话'}</div>
        <div class="text-[11px] mt-1 leading-relaxed">${filtered ? '试试清空筛选条件，或点击「同步消息」拉取最新会话' : '点击上方「同步消息」从闲鱼 IM 拉取最新会话'}</div>
      </div>
    `;
    return;
  }

  listEl.innerHTML = conversations.map((conversation) => {
    const id = opsSafeInt(conversation.id);
    const active = Number(state.activeConversationId) === id;
    const buyerNick = opsDecode(conversation.buyer_nickname) || conversation.buyer_id || '未知买家';
    const avatar = opsEscape(Array.from(buyerNick)[0] || '💬');
    const avatarUrl = String(conversation.buyer_avatar || '').trim();
    const avatarHtml = avatarUrl
      ? `<div class="relative w-9 h-9 rounded-full bg-indigo-100 text-indigo-600 flex items-center justify-center text-xs font-bold shrink-0 overflow-hidden">
           <span>${avatar}</span>
           <img src="${opsEscape(avatarUrl)}" alt="" loading="lazy" referrerpolicy="no-referrer"
                class="absolute inset-0 w-full h-full object-cover" onerror="this.remove()" />
         </div>`
      : `<div class="w-9 h-9 rounded-full bg-indigo-100 text-indigo-600 flex items-center justify-center text-xs font-bold shrink-0">${avatar}</div>`;
    const itemId = String(conversation.xianyu_item_id || '').trim();
    const itemTitle = opsDecode(conversation.item_title) || '';
    const itemLabel = itemTitle || (itemId ? `宝贝 ${itemId}` : '');
    const itemHtml = itemId
      ? `<a href="${opsEscape(opsItemUrl(itemId))}" target="_blank" rel="noopener noreferrer"
            onclick="event.stopPropagation()"
            class="px-1.5 py-0.5 rounded bg-indigo-50 text-indigo-600 hover:bg-indigo-100 text-[10px] truncate max-w-[160px]"
            title="在闲鱼打开该宝贝">🏷 ${opsEscape(itemLabel)}</a>`
      : '';
    const account = state.accounts.find((item) => Number(item.id) === Number(conversation.account_id));
    const accountName = account
      ? opsAccountName(account, conversation.account_id)
      : (opsDecode(conversation.seller_nick) || (conversation.account_id ? `账号 #${opsSafeInt(conversation.account_id)}` : '未知账号'));
    const unread = Math.max(0, Number(conversation.unread_count) || 0);
    const isOpen = conversation.status === 'open';

    return `
      <div role="button" tabindex="0" onclick="opsOpenConversation(${id})"
        onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();opsOpenConversation(${id});}"
        class="w-full text-left px-4 py-3 border-l-4 transition cursor-pointer ${active ? 'bg-indigo-50/70 border-indigo-500' : 'border-transparent hover:bg-slate-50'}">
        <div class="flex items-start gap-3">
          ${avatarHtml}
          <div class="min-w-0 flex-1">
            <div class="flex items-center justify-between gap-2">
              <span class="text-xs font-bold text-slate-900 truncate">${opsEscape(buyerNick)}</span>
              <span class="text-[10px] text-slate-400 shrink-0">${opsEscape(opsFmtShortTime(conversation.last_message_at || conversation.updated_at || conversation.created_at))}</span>
            </div>
            <div class="flex items-center justify-between gap-2 mt-0.5">
              <span class="text-[11px] ${unread > 0 ? 'text-slate-700 font-medium' : 'text-slate-500'} truncate">${opsEscape(conversation.last_message || '（暂无消息）')}</span>
              ${unread > 0
                ? `<span class="min-w-[18px] h-[18px] px-1 rounded-full bg-rose-500 text-white text-[10px] font-bold flex items-center justify-center shrink-0">${unread > 99 ? '99+' : unread}</span>`
                : ''}
            </div>
            <div class="flex items-center gap-1.5 mt-1 flex-wrap">
              <span class="px-1.5 py-0.5 rounded bg-slate-100 text-slate-500 text-[10px] truncate max-w-[140px]">${opsEscape(accountName)}</span>
              <span class="text-[10px] ${isOpen ? 'text-emerald-600' : 'text-slate-400'}">${isOpen ? '进行中' : '已关闭'}</span>
              ${itemHtml}
            </div>
          </div>
        </div>
      </div>
    `;
  }).join('');
}

function opsMessagesRenderListError(message) {
  const listEl = document.getElementById('ops-msg-list');
  if (!listEl) return;
  listEl.innerHTML = `
    <div class="px-5 py-12 text-center">
      <div class="text-3xl mb-2">⚠️</div>
      <div class="text-xs font-medium text-rose-600">会话列表加载失败</div>
      <div class="text-[11px] text-slate-400 mt-1 break-all">${opsEscape(message || '请稍后重试')}</div>
      <button type="button" onclick="opsMessagesRefresh()"
        class="mt-3 px-3 py-1.5 rounded-lg bg-slate-900 text-white text-xs font-semibold">重试</button>
    </div>
  `;
}

function opsMessagesRenderThreadPlaceholder(message) {
  const thread = document.getElementById('ops-msg-thread');
  if (thread) {
    thread.innerHTML = `
      <div class="h-full flex flex-col items-center justify-center text-center text-slate-400">
        <div class="text-3xl mb-2">💬</div>
        <div class="text-xs">${opsEscape(message || '点击左侧会话查看聊天记录')}</div>
      </div>
    `;
  }
  opsSetText('ops-msg-active-title', '请选择左侧会话');
  opsSetText('ops-msg-active-meta', '点击会话查看聊天记录并回复买家');
  opsSetHidden('ops-msg-composer', true);
  opsSetHidden('ops-msg-delete', true);
}

function opsMessagesRenderHeader(conversation) {
  const state = opsMessagesState;
  const data = conversation || {};
  const buyerNick = opsDecode(data.buyer_nickname) || data.buyer_id || '未知买家';
  const account = state.accounts.find((item) => Number(item.id) === Number(data.account_id));
  const accountName = account
    ? opsAccountName(account, data.account_id)
    : (opsDecode(data.seller_nick) || (data.account_id ? `账号 #${opsSafeInt(data.account_id)}` : '未知账号'));
  const statusText = data.status === 'closed' ? '已关闭' : '进行中';

  const itemId = String(data.xianyu_item_id || '').trim();
  const itemTitle = opsDecode(data.item_title) || '';
  const metaEl = document.getElementById('ops-msg-active-meta');
  const itemHtml = itemId
    ? ` · <a href="${opsEscape(opsItemUrl(itemId))}" target="_blank" rel="noopener noreferrer"
          class="text-indigo-600 hover:underline" title="在闲鱼打开该宝贝">🏷 ${opsEscape(itemTitle || `宝贝 ${itemId}`)}</a>`
    : '';

  opsSetText('ops-msg-active-title', `${buyerNick}${data.buyer_id ? ` · ${data.buyer_id}` : ''}`);
  if (metaEl) {
    metaEl.innerHTML = `账号：${opsEscape(accountName)} · 会话：${opsEscape(data.session_cid || '-')} · ${opsEscape(statusText)}${itemHtml}`;
  }
  opsSetHidden('ops-msg-delete', false);
}

function opsMessagesRenderThread(messages) {
  const thread = document.getElementById('ops-msg-thread');
  if (!thread) return;
  const list = Array.isArray(messages) ? messages : [];

  if (list.length === 0) {
    thread.innerHTML = `
      <div class="h-full flex flex-col items-center justify-center text-center text-slate-400">
        <div class="text-3xl mb-2">📝</div>
        <div class="text-xs">暂无聊天记录</div>
        <div class="text-[11px] mt-1">可在下方输入框回复买家</div>
      </div>
    `;
    return;
  }

  thread.innerHTML = list.map((message) => {
    const isOut = message.direction === 'out';
    const sender = isOut
      ? '我'
      : (opsDecode(message.sender_nick) || (message.sender_id ? `买家 ${message.sender_id}` : '买家'));
    const typeLabel = message.msg_type && message.msg_type !== 'text'
      ? `<span class="ml-1 px-1.5 py-0.5 rounded bg-slate-200/70 text-slate-500 text-[9px]">${opsEscape(message.msg_type)}</span>`
      : '';
    return `
      <div class="flex ${isOut ? 'justify-end' : 'justify-start'}">
        <div class="max-w-[78%] space-y-1">
          <div class="px-3.5 py-2 rounded-2xl text-sm whitespace-pre-wrap break-words ${isOut
            ? 'bg-indigo-600 text-white rounded-tr-sm'
            : 'bg-white border border-slate-200 text-slate-700 rounded-tl-sm'}">${opsEscape(message.content || '')}</div>
          <div class="text-[10px] text-slate-400 ${isOut ? 'text-right' : 'text-left'}">
            ${opsEscape(sender)} · ${opsEscape(opsFmtTime(message.created_at))}${typeLabel}
          </div>
        </div>
      </div>
    `;
  }).join('');

  thread.scrollTop = thread.scrollHeight;
}

function opsMessagesBuildQuery() {
  const state = opsMessagesState;
  const params = new URLSearchParams();
  if (state.accountId) params.set('account_id', state.accountId);
  if (state.status) params.set('status', state.status);
  if (state.search) params.set('search', state.search);
  params.set('limit', '100');
  return params.toString();
}

async function opsMessagesLoadAccounts() {
  const accounts = await opsLoadXianyuAccounts();
  opsMessagesState.accounts = accounts;
  const selected = opsFillAccountSelect('ops-msg-account', accounts, opsMessagesState.accountId, '全部账号');
  opsMessagesState.accountId = selected;
}

/**
 * 会话列表 + 统计刷新。
 * options.silent = true 时用于 20 秒轮询，不弹出错误 toast。
 */
async function opsMessagesRefreshData(options) {
  const opts = options || {};
  const state = opsMessagesState;
  if (!document.getElementById('ops-msg-list')) return;

  const silent = !!opts.silent;
  const seq = ++state.listSeq;
  const query = opsMessagesBuildQuery();

  const results = await Promise.allSettled([
    opsRequest('/api/messages/stats'),
    opsRequest(`/api/messages/conversations?${query}`)
  ]);
  if (seq !== state.listSeq) return;

  const authFailed = results.some((result) => result.status === 'rejected' && opsIsAuthError(result.reason));
  if (authFailed) {
    opsSetAuthState('msg', true);
    return;
  }
  opsSetAuthState('msg', false);

  let errorMessage = '';

  if (results[0].status === 'fulfilled') {
    state.stats = results[0].value || {};
    opsMessagesRenderStats(state.stats);
  } else if (!silent) {
    errorMessage = (results[0].reason && results[0].reason.message) || '消息统计加载失败';
  }

  if (results[1].status === 'fulfilled') {
    const data = results[1].value;
    const list = Array.isArray(data) ? data : (data && Array.isArray(data.items) ? data.items : []);
    state.conversations = list;
    opsMessagesRenderList();
    if (state.activeConversationId) {
      const active = list.find((item) => Number(item.id) === Number(state.activeConversationId));
      if (active) state.activeConversation = Object.assign({}, state.activeConversation || {}, active);
    }
  } else if (!silent) {
    opsMessagesRenderListError((results[1].reason && results[1].reason.message) || '会话列表加载失败');
  }

  if (errorMessage) opsToast(errorMessage, true);

  if (state.activeConversationId) {
    // 发送消息后调用方会单独刷新聊天面板，避免重复请求。
    if (!opts.skipThread) {
      await opsMessagesLoadThread(state.activeConversationId, { markRead: false, silent: true });
    }
  } else if (document.getElementById('ops-msg-thread')) {
    opsMessagesRenderThreadPlaceholder('点击左侧会话查看聊天记录');
  }
}

async function opsMessagesLoadThread(id, options) {
  const opts = options || {};
  const state = opsMessagesState;
  const conversationId = opsSafeInt(id);
  if (conversationId <= 0) return;

  const seq = ++state.threadSeq;
  try {
    const data = await opsRequest(`/api/messages/conversations/${conversationId}`);
    if (seq !== state.threadSeq) return;
    if (Number(state.activeConversationId) !== conversationId) return;

    const conversation = (data && data.conversation) || {};
    const messages = data && Array.isArray(data.messages) ? data.messages : [];

    state.activeConversation = conversation;
    const index = state.conversations.findIndex((item) => Number(item.id) === conversationId);
    if (index >= 0) {
      state.conversations[index] = Object.assign({}, state.conversations[index], conversation);
      opsMessagesRenderList();
    }

    opsMessagesRenderHeader(conversation);
    opsMessagesRenderThread(messages);
    opsSetHidden('ops-msg-composer', false);
    opsSetHidden('ops-msg-delete', false);

    if (opts.markRead) {
      try {
        await opsMessagesMarkRead(conversationId);
      } catch (error) {
        if (opsHandleAuthError('msg', error)) return;
        if (!opts.silent) opsToast(error.message || '标记已读失败', true);
      }
    }
  } catch (error) {
    if (seq !== state.threadSeq) return;
    if (opsHandleAuthError('msg', error)) return;
    if (!opts.silent) opsToast(error.message || '聊天记录加载失败', true);
    const thread = document.getElementById('ops-msg-thread');
    if (thread) {
      thread.innerHTML = `
        <div class="h-full flex flex-col items-center justify-center text-center">
          <div class="text-3xl mb-2">⚠️</div>
          <div class="text-xs font-medium text-rose-600">聊天记录加载失败</div>
          <div class="text-[11px] text-slate-400 mt-1 break-all max-w-md">${opsEscape(error.message || '请稍后重试')}</div>
          <button type="button" onclick="opsOpenConversation(${conversationId})"
            class="mt-3 px-3 py-1.5 rounded-lg bg-slate-900 text-white text-xs font-semibold">重试</button>
        </div>
      `;
    }
  }
}

async function opsMessagesMarkRead(id) {
  const conversationId = opsSafeInt(id);
  await opsRequest(`/api/messages/conversations/${conversationId}/read`, { method: 'POST' });

  const state = opsMessagesState;
  const conversation = state.conversations.find((item) => Number(item.id) === conversationId);
  const previousUnread = conversation ? Math.max(0, Number(conversation.unread_count) || 0) : 0;
  if (conversation) conversation.unread_count = 0;
  if (state.activeConversation && Number(state.activeConversation.id) === conversationId) {
    state.activeConversation.unread_count = 0;
  }
  opsMessagesRenderList();

  // 先本地扣减未读，再拉取统计接口校准，避免统计接口稍慢时红点仍然存在。
  if (state.stats) {
    state.stats.unread = Math.max(0, (Number(state.stats.unread) || 0) - previousUnread);
    opsMessagesRenderStats(state.stats);
  }

  // 刷新统计里的未读数，保证顶部未读卡片准确。
  try {
    const stats = await opsRequest('/api/messages/stats');
    state.stats = stats || {};
    opsMessagesRenderStats(state.stats);
  } catch (error) {
    if (opsHandleAuthError('msg', error)) return;
  }
}

function opsMessagesResetActive() {
  opsMessagesState.activeConversationId = null;
  opsMessagesState.activeConversation = null;
  opsMessagesState.threadSeq += 1;
  opsMessagesRenderThreadPlaceholder('点击左侧会话查看聊天记录');
}

function opsMessagesReadFilterInputs() {
  const state = opsMessagesState;
  const accountEl = document.getElementById('ops-msg-account');
  const statusEl = document.getElementById('ops-msg-status');
  const searchEl = document.getElementById('ops-msg-search');
  if (accountEl) state.accountId = accountEl.value || '';
  if (statusEl) state.status = statusEl.value || '';
  if (searchEl) state.search = searchEl.value.trim();
}

function opsMessagesAccountChanged(value) {
  opsMessagesState.accountId = value || '';
  opsMessagesReadFilterInputs();
  opsMessagesResetActive();
  opsMessagesRefresh();
}

function opsMessagesStatusChanged(value) {
  opsMessagesState.status = value || '';
  opsMessagesReadFilterInputs();
  opsMessagesResetActive();
  opsMessagesRefresh();
}

function opsMessagesSearchKeydown(event) {
  if (event && event.key === 'Enter' && !event.isComposing) {
    event.preventDefault();
    opsMessagesApplyFilters();
  }
}

function opsMessagesApplyFilters() {
  const before = JSON.stringify([opsMessagesState.accountId, opsMessagesState.status, opsMessagesState.search]);
  opsMessagesReadFilterInputs();
  const after = JSON.stringify([opsMessagesState.accountId, opsMessagesState.status, opsMessagesState.search]);
  if (before !== after) opsMessagesResetActive();
  return opsMessagesRefresh();
}

function opsMessagesRefresh() {
  return opsMessagesRefreshData({ silent: false }).catch((error) => {
    if (!opsHandleAuthError('msg', error)) opsToast(error.message || '消息刷新失败', true);
  });
}

async function opsMessagesSync() {
  const state = opsMessagesState;
  if (state.syncing) return;
  state.syncing = true;
  const button = document.getElementById('ops-msg-sync-btn');
  const originalText = button ? button.textContent : '';
  if (button) {
    button.disabled = true;
    button.textContent = '同步中...';
    button.classList.add('opacity-60', 'cursor-not-allowed');
  }

  try {
    const accountId = state.accountId ? Number(state.accountId) : null;
    const result = await opsRequest('/api/messages/sync', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ account_id: Number.isFinite(accountId) ? accountId : null })
    });
    const errors = result && Array.isArray(result.errors) ? result.errors : [];
    const message = (result && result.message) || `已同步 ${opsCount(result && result.synced_accounts)} 个账号的 ${opsCount(result && result.synced_conversations)} 个会话`;
    const suffix = errors.length ? `（${errors.length} 个账号同步失败）` : '';
    opsToast(`${message}${suffix}`, errors.length > 0);
    await opsMessagesRefreshData({ silent: false });
  } catch (error) {
    if (!opsHandleAuthError('msg', error)) opsToast(error.message || '消息同步失败', true);
  } finally {
    state.syncing = false;
    if (button) {
      button.disabled = false;
      button.textContent = originalText;
      button.classList.remove('opacity-60', 'cursor-not-allowed');
    }
  }
}

async function opsOpenConversation(id) {
  const state = opsMessagesState;
  const conversationId = opsSafeInt(id);
  if (conversationId <= 0) return;

  state.activeConversationId = conversationId;
  state.activeConversation = state.conversations.find((item) => Number(item.id) === conversationId) || null;
  opsMessagesRenderList();
  if (state.activeConversation) opsMessagesRenderHeader(state.activeConversation);

  const thread = document.getElementById('ops-msg-thread');
  if (thread) {
    thread.innerHTML = '<div class="h-full flex items-center justify-center text-xs text-slate-400">正在加载聊天记录...</div>';
  }
  opsSetHidden('ops-msg-composer', true);
  opsSetHidden('ops-msg-delete', true);

  await opsMessagesLoadThread(conversationId, { markRead: true, silent: false });
}

async function opsMessagesSend() {
  const state = opsMessagesState;
  const conversationId = opsSafeInt(state.activeConversationId);
  if (conversationId <= 0) {
    opsToast('请先选择一个会话', true);
    return;
  }
  if (state.sending) return;

  const input = document.getElementById('ops-msg-input');
  const content = input ? input.value.trim() : '';
  if (!content) {
    opsToast('请输入回复内容', true);
    if (input) input.focus();
    return;
  }

  state.sending = true;
  const button = document.getElementById('ops-msg-send-btn');
  const originalText = button ? button.textContent : '';
  if (button) {
    button.disabled = true;
    button.textContent = '发送中...';
    button.classList.add('opacity-60', 'cursor-not-allowed');
  }

  try {
    const result = await opsRequest(`/api/messages/conversations/${conversationId}/send`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ content })
    });
    if (input) input.value = '';
    const remoteSent = !result || result.remote_sent !== false;
    const message = (result && result.message_text) || (remoteSent ? '消息已发送' : '消息已保存，但远端发送失败');
    opsToast(message, !remoteSent);

    // 发送后刷新消息与左侧会话的最后一条消息。
    await opsMessagesLoadThread(conversationId, { markRead: false, silent: true });
    await opsMessagesRefreshData({ silent: true, skipThread: true });
  } catch (error) {
    if (!opsHandleAuthError('msg', error)) opsToast(error.message || '消息发送失败', true);
  } finally {
    state.sending = false;
    if (button) {
      button.disabled = false;
      button.textContent = originalText;
      button.classList.remove('opacity-60', 'cursor-not-allowed');
    }
    if (input) input.focus();
  }
}

function opsMessagesInputKeydown(event) {
  if (event && event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    opsMessagesSend();
  }
}

async function opsDeleteConversation() {
  const state = opsMessagesState;
  const conversationId = opsSafeInt(state.activeConversationId);
  if (conversationId <= 0) return;
  if (!window.confirm('确认删除该会话及其聊天记录？该操作不可撤销。')) return;

  try {
    await opsRequest(`/api/messages/conversations/${conversationId}`, { method: 'DELETE' });
    opsMessagesResetActive();
    opsToast('会话已删除');
    await opsMessagesRefreshData({ silent: true });
  } catch (error) {
    if (!opsHandleAuthError('msg', error)) opsToast(error.message || '删除会话失败', true);
  }
}

/** 首次进入注册 20 秒轮询；只在 messages 页可见时静默刷新。 */
function opsMessagesEnsureTimer() {
  if (opsMessagesState.timerStarted) return;
  opsMessagesState.timerStarted = true;

  window.setInterval(() => {
    try {
      let isCurrent = false;
      try {
        isCurrent = typeof currentTab !== 'undefined' && currentTab === 'messages';
      } catch (_) {
        isCurrent = false;
      }
      if (!isCurrent || document.hidden) return;
      const root = document.getElementById('view-messages');
      if (!root || root.classList.contains('hidden') || !document.getElementById('ops-msg-list')) return;
      // 登录卡片展示时不需要轮询。
      const authNotice = document.getElementById('ops-msg-auth');
      if (authNotice && !authNotice.classList.contains('hidden')) return;
      // 轮询只刷新会话列表与统计，避免聊天面板每 20 秒被重新渲染、打断阅读。
      opsMessagesRefreshData({ silent: true, skipThread: true }).catch(() => {});
    } catch (_) {
      /* 静默轮询不抛出未捕获异常 */
    }
  }, 20000);
}

/** 在线消息页面入口（注册到 window.pageLoaders.messages）。 */
async function loadOpsMessages() {
  if (!opsMessagesEnsureSkeleton()) return;

  try {
    try {
      await opsMessagesLoadAccounts();
    } catch (error) {
      if (opsHandleAuthError('msg', error)) return;
    }

    await opsMessagesRefreshData({ silent: false });
    opsMessagesEnsureTimer();
  } catch (error) {
    if (!opsHandleAuthError('msg', error)) opsToast(error.message || '在线消息页面加载失败', true);
  }
}

/* =========================================================================
 * 三、自动发货（#view-autodeliver）
 * ========================================================================= */

var opsAutoDeliverState = {
  rendered: false,
  config: null,
  stats: null,
  readiness: [],
  scanning: false
};

function opsAutoDeliverEnsureSkeleton() {
  const root = document.getElementById('view-autodeliver');
  if (!root) return false;
  if (opsAutoDeliverState.rendered && root.querySelector('#ops-ad-body')) return true;

  root.innerHTML = `
    <div class="space-y-4">
      ${opsAuthNoticeHtml('ad', '登录后可查看自动发货配置、就绪检查与扫描统计。')}

      <div id="ops-ad-body" class="space-y-4">
        <div class="grid grid-cols-2 lg:grid-cols-5 gap-3 sm:gap-4">
          <div class="bg-white rounded-xl p-4 border border-emerald-200 shadow-sm">
            <div class="text-xs font-medium text-emerald-600">✅ 今日成功</div>
            <div id="ops-ad-stat-success" class="text-2xl font-bold text-emerald-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">今天自动发货成功</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-rose-200 shadow-sm">
            <div class="text-xs font-medium text-rose-600">❌ 今日失败</div>
            <div id="ops-ad-stat-failed" class="text-2xl font-bold text-rose-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">今天自动发货失败</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-indigo-200 shadow-sm">
            <div class="text-xs font-medium text-indigo-600">📈 成功率</div>
            <div id="ops-ad-stat-rate" class="text-2xl font-bold text-indigo-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">成功 / 全部发货尝试</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-amber-200 shadow-sm">
            <div class="text-xs font-medium text-amber-600">⏳ 待发货订单</div>
            <div id="ops-ad-stat-pending" class="text-2xl font-bold text-amber-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">等待自动扫描处理</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-slate-200 shadow-sm">
            <div class="text-xs font-medium text-slate-500">🔑 可用卡密</div>
            <div id="ops-ad-stat-codes" class="text-2xl font-bold text-slate-900 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">全部商品未使用卡密</div>
          </div>
        </div>

        <div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">
          <div class="px-5 py-4 border-b border-slate-200 flex items-center justify-between gap-3">
            <div>
              <h2 class="text-base font-bold text-slate-900">自动发货配置</h2>
              <p class="text-xs text-slate-500 mt-0.5">控制后台扫描、远端虚拟发货、IM 消息与通知行为</p>
            </div>
            <span id="ops-ad-config-state" class="shrink-0 text-[11px] px-2.5 py-1 rounded-full bg-slate-100 text-slate-500 font-medium">加载中...</span>
          </div>

          <div class="p-5 space-y-4">
            <div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-3">
              <label class="flex items-start gap-2.5 p-3 rounded-xl border border-slate-200 bg-slate-50/60 cursor-pointer hover:bg-slate-50">
                <input id="ops-ad-enabled" type="checkbox" class="mt-0.5 w-4 h-4 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500" />
                <span>
                  <span class="block text-xs font-bold text-slate-800">启用自动发货</span>
                  <span class="block text-[11px] text-slate-500 mt-0.5">关闭后调度器会跳过自动扫描</span>
                </span>
              </label>
              <label class="flex items-start gap-2.5 p-3 rounded-xl border border-slate-200 bg-slate-50/60 cursor-pointer hover:bg-slate-50">
                <input id="ops-ad-auto-consign" type="checkbox" class="mt-0.5 w-4 h-4 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500" />
                <span>
                  <span class="block text-xs font-bold text-slate-800">自动确认闲鱼发货</span>
                  <span class="block text-[11px] text-slate-500 mt-0.5">调用远端虚拟发货接口标记已发货</span>
                </span>
              </label>
              <label class="flex items-start gap-2.5 p-3 rounded-xl border border-slate-200 bg-slate-50/60 cursor-pointer hover:bg-slate-50">
                <input id="ops-ad-auto-send-message" type="checkbox" class="mt-0.5 w-4 h-4 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500" />
                <span>
                  <span class="block text-xs font-bold text-slate-800">自动发送卡密消息</span>
                  <span class="block text-[11px] text-slate-500 mt-0.5">通过闲鱼 IM 把发货内容发给买家</span>
                </span>
              </label>
              <label class="flex items-start gap-2.5 p-3 rounded-xl border border-slate-200 bg-slate-50/60 cursor-pointer hover:bg-slate-50">
                <input id="ops-ad-notify-success" type="checkbox" class="mt-0.5 w-4 h-4 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500" />
                <span>
                  <span class="block text-xs font-bold text-slate-800">成功时发送通知</span>
                  <span class="block text-[11px] text-slate-500 mt-0.5">发货成功后按通知规则推送</span>
                </span>
              </label>
              <label class="flex items-start gap-2.5 p-3 rounded-xl border border-slate-200 bg-slate-50/60 cursor-pointer hover:bg-slate-50">
                <input id="ops-ad-notify-failure" type="checkbox" class="mt-0.5 w-4 h-4 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500" />
                <span>
                  <span class="block text-xs font-bold text-slate-800">失败时发送通知</span>
                  <span class="block text-[11px] text-slate-500 mt-0.5">发货失败后按通知规则推送</span>
                </span>
              </label>
              <label class="flex items-start gap-2.5 p-3 rounded-xl border border-amber-200 bg-amber-50/60 cursor-pointer hover:bg-amber-50">
                <input id="ops-ad-dry-run" type="checkbox" class="mt-0.5 w-4 h-4 rounded border-slate-300 text-amber-600 focus:ring-amber-500" />
                <span>
                  <span class="block text-xs font-bold text-amber-800">演练模式 (dry_run)</span>
                  <span class="block text-[11px] text-amber-700/80 mt-0.5">只扫描与模拟，不真正发货、不扣卡密</span>
                </span>
              </label>
            </div>

            <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
              <div>
                <label class="block text-xs font-bold text-slate-700 mb-1">扫描间隔（秒）</label>
                <input id="ops-ad-scan-interval" type="number" min="5" step="1"
                  class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono focus:outline-none focus:border-indigo-500" />
                <p class="text-[11px] text-slate-400 mt-1">保存后同步到定时任务 xianyu_auto_deliver</p>
              </div>
              <div>
                <label class="block text-xs font-bold text-slate-700 mb-1">失败重试次数</label>
                <input id="ops-ad-retry-limit" type="number" min="0" step="1"
                  class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono focus:outline-none focus:border-indigo-500" />
                <p class="text-[11px] text-slate-400 mt-1">发货失败后的最大重试次数</p>
              </div>
              <div>
                <label class="block text-xs font-bold text-slate-700 mb-1">低库存阈值（可用卡密）</label>
                <input id="ops-ad-low-stock" type="number" min="0" step="1"
                  class="w-full rounded-lg border border-slate-300 px-3 py-2 text-sm font-mono focus:outline-none focus:border-indigo-500" />
                <p class="text-[11px] text-slate-400 mt-1">可用卡密低于该值时触发库存预警</p>
              </div>
            </div>

            <div>
              <label class="block text-xs font-bold text-slate-700 mb-1">自动回复模板</label>
              <textarea id="ops-ad-reply-template" rows="3"
                placeholder="例如：【自动发货】您好，您的卡密是：{code}，请尽快使用。"
                class="w-full rounded-lg border border-slate-300 px-3 py-2 text-xs font-mono resize-y focus:outline-none focus:border-indigo-500"></textarea>
              <p class="text-[11px] text-slate-400 mt-1">发货后自动发送给买家的消息内容，可结合商品模板使用。</p>
            </div>

            <div class="flex flex-wrap items-center justify-between gap-3 pt-3 border-t border-slate-100">
              <p class="text-[11px] text-slate-400 max-w-xl leading-relaxed">
                后台定时任务会自动执行自动发货扫描；关闭「启用自动发货」后，调度器会跳过自动扫描，手动「立即扫描」仍可执行一次。
              </p>
              <button type="button" id="ops-ad-save" onclick="opsAutoDeliverSave()"
                class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold shadow-xs">
                💾 保存配置
              </button>
            </div>
          </div>
        </div>

        <div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">
          <div class="px-5 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-3">
            <div>
              <h2 class="text-base font-bold text-slate-900">自动发货就绪检查</h2>
              <p class="text-xs text-slate-500 mt-0.5">检查卡密商品、发货模式、库存、闲鱼宝贝绑定与账号状态</p>
            </div>
            <div class="flex flex-wrap items-center gap-2">
              <span id="ops-ad-readiness-count" class="text-[11px] text-slate-400">正在检查...</span>
              <button type="button" onclick="opsAutoDeliverRefresh()"
                class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-semibold border border-slate-300">
                刷新
              </button>
              <button type="button" id="ops-ad-scan" onclick="opsAutoDeliverScan()"
                class="px-3.5 py-1.5 rounded-lg bg-emerald-600 hover:bg-emerald-700 text-white text-xs font-semibold shadow-xs">
                ⚡ 立即扫描
              </button>
            </div>
          </div>

          <div id="ops-ad-scan-result" class="hidden m-4 rounded-xl border border-indigo-200 bg-indigo-50/60 px-4 py-3 text-xs text-slate-700 space-y-1.5"></div>

          <div class="overflow-x-auto">
            <table class="w-full text-left border-collapse min-w-[980px]">
              <thead>
                <tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">
                  <th class="py-3 px-5">商品</th>
                  <th class="py-3 px-3">发货模式</th>
                  <th class="py-3 px-3">可用卡密</th>
                  <th class="py-3 px-4">绑定闲鱼宝贝</th>
                  <th class="py-3 px-3">账号</th>
                  <th class="py-3 px-3">状态</th>
                  <th class="py-3 px-5">问题</th>
                </tr>
              </thead>
              <tbody id="ops-ad-readiness-tbody" class="divide-y divide-slate-100 text-sm">
                <tr><td colspan="7" class="py-10 text-center text-xs text-slate-400">正在检查自动发货就绪状态...</td></tr>
              </tbody>
            </table>
          </div>
        </div>

        <div class="rounded-xl border border-indigo-200 bg-indigo-50/60 px-4 py-3 text-xs text-slate-700 flex items-start gap-2.5">
          <span class="text-base leading-none">ℹ️</span>
          <p class="leading-relaxed">
            后台定时任务会自动执行自动发货扫描。关闭「启用自动发货」后，调度器会跳过自动扫描；保存扫描间隔后，会同步更新定时任务
            <span class="font-mono text-indigo-700">xianyu_auto_deliver</span> 的 interval_seconds。
          </p>
        </div>
      </div>
    </div>
  `;

  opsAutoDeliverState.rendered = true;
  return true;
}

function opsAutoDeliverRenderStats(stats) {
  const data = stats || {};
  opsSetText('ops-ad-stat-success', opsCount(data.today_delivered));
  opsSetText('ops-ad-stat-failed', opsCount(data.today_failed));
  opsSetText('ops-ad-stat-rate', opsPercent(data.success_rate));
  opsSetText('ops-ad-stat-pending', opsCount(data.pending_orders));
  opsSetText('ops-ad-stat-codes', opsCount(data.unused_codes));
}

function opsAutoDeliverFillConfig(config) {
  const data = config || {};
  opsAutoDeliverState.config = data;

  opsSetChecked('ops-ad-enabled', data.enabled);
  opsSetChecked('ops-ad-auto-consign', data.auto_consign);
  opsSetChecked('ops-ad-auto-send-message', data.auto_send_message);
  opsSetChecked('ops-ad-notify-success', data.notify_on_success);
  opsSetChecked('ops-ad-notify-failure', data.notify_on_failure);
  opsSetChecked('ops-ad-dry-run', data.dry_run);

  const interval = Number(data.scan_interval_seconds);
  const retryLimit = Number(data.retry_limit);
  const lowStock = Number(data.low_stock_threshold);
  opsSetValue('ops-ad-scan-interval', Number.isFinite(interval) ? interval : 45);
  opsSetValue('ops-ad-retry-limit', Number.isFinite(retryLimit) ? retryLimit : 3);
  opsSetValue('ops-ad-low-stock', Number.isFinite(lowStock) ? lowStock : 5);
  opsSetValue('ops-ad-reply-template', data.reply_template || '');

  const stateEl = document.getElementById('ops-ad-config-state');
  if (stateEl) {
    stateEl.textContent = data.enabled ? '已启用' : '已停用';
    stateEl.className = data.enabled
      ? 'shrink-0 text-[11px] px-2.5 py-1 rounded-full bg-emerald-50 text-emerald-700 font-semibold border border-emerald-200'
      : 'shrink-0 text-[11px] px-2.5 py-1 rounded-full bg-slate-100 text-slate-500 font-semibold border border-slate-200';
  }
}

function opsAutoDeliverTemplateLabel(type) {
  const labels = {
    redeem_code: '一单一码卡密',
    file_link: '固定链接 / 文本',
    steam_gift: 'Steam 礼物'
  };
  return labels[type] || type || '-';
}

function opsAutoDeliverRenderReadiness(items) {
  const state = opsAutoDeliverState;
  const rows = Array.isArray(items) ? items : [];
  state.readiness = rows;

  const tbody = document.getElementById('ops-ad-readiness-tbody');
  if (!tbody) return;

  const readyCount = rows.filter((item) => item && item.ready === true).length;
  opsSetText('ops-ad-readiness-count', `${readyCount} / ${rows.length} 个商品就绪`);

  if (rows.length === 0) {
    tbody.innerHTML = `
      <tr>
        <td colspan="7" class="py-12 text-center">
          <div class="text-3xl mb-2">📦</div>
          <div class="text-xs font-medium text-slate-500">暂无自动发货商品</div>
          <div class="text-[11px] text-slate-400 mt-1">请先在「商品」页面创建卡密商品，并绑定闲鱼宝贝与账号</div>
          <button type="button" onclick="if (typeof switchTab === 'function') switchTab('products')"
            class="mt-3 px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold">前往商品页</button>
        </td>
      </tr>
    `;
    return;
  }

  const threshold = Number(state.config && state.config.low_stock_threshold);

  tbody.innerHTML = rows.map((item) => {
    const row = item || {};
    const productId = opsSafeInt(row.product_id);
    const productName = row.product_name || `卡密商品 #${productId}`;
    const unused = Number(row.unused_codes);
    const unusedCount = Number.isFinite(unused) ? unused : 0;
    const productDisabled = row.enabled === false;
    const ready = row.ready === true;

    let stockClass = 'text-emerald-600';
    if (unusedCount <= 0) stockClass = 'text-rose-600';
    else if (Number.isFinite(threshold) && threshold > 0 && unusedCount <= threshold) stockClass = 'text-amber-600';

    const itemStatusText = row.item_status === 'on_sale'
      ? '在售'
      : (row.item_status === 'off_shelf' ? '已下架' : (row.item_status || ''));
    const itemStatus = itemStatusText
      ? `<div class="text-[10px] ${row.item_status === 'on_sale' ? 'text-emerald-600' : 'text-rose-500'} mt-0.5">${opsEscape(itemStatusText)}</div>`
      : '';

    const bindCell = row.xianyu_item_id
      ? `<div class="font-mono text-[11px] text-slate-700">${opsEscape(row.xianyu_item_id)}</div>
         ${row.xianyu_title ? `<div class="text-[10px] text-slate-400 truncate max-w-[190px]" title="${opsEscape(row.xianyu_title)}">${opsEscape(row.xianyu_title)}</div>` : ''}
         ${itemStatus}`
      : `<span class="text-[11px] text-rose-600">未绑定</span>${itemStatus}`;

    const accountCell = row.account_id
      ? `<div class="text-xs font-medium text-slate-800">${opsEscape(opsDecode(row.account_nick) || `账号 #${opsSafeInt(row.account_id)}`)}</div>
         <div class="text-[10px] text-slate-400 font-mono">#${opsSafeInt(row.account_id)}</div>`
      : '<span class="text-[11px] text-rose-600">未绑定</span>';

    return `
      <tr class="hover:bg-slate-50/80 transition">
        <td class="py-3 px-5">
          <div class="flex items-center gap-2">
            <div class="font-semibold text-slate-900">${opsEscape(productName)}</div>
            ${productDisabled ? '<span class="px-1.5 py-0.5 rounded bg-amber-50 text-amber-700 text-[10px] font-semibold">已禁用</span>' : ''}
          </div>
          <div class="text-[10px] text-slate-400 font-mono mt-0.5">商品 #${productId}</div>
        </td>
        <td class="py-3 px-3 text-xs text-slate-700">
          <span class="px-2 py-0.5 rounded bg-slate-100 text-slate-600 text-[11px]">${opsEscape(opsAutoDeliverTemplateLabel(row.template_type))}</span>
        </td>
        <td class="py-3 px-3">
          <span class="font-mono font-bold ${stockClass}">${unusedCount}</span>
          <span class="text-[10px] text-slate-400"> 条</span>
        </td>
        <td class="py-3 px-4">${bindCell}</td>
        <td class="py-3 px-3">${accountCell}</td>
        <td class="py-3 px-3">
          ${ready
            ? '<span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-emerald-50 text-emerald-700 border border-emerald-200 text-[11px] font-semibold">✅ 就绪</span>'
            : '<span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-rose-50 text-rose-700 border border-rose-200 text-[11px] font-semibold">⛔ 未就绪</span>'}
        </td>
        <td class="py-3 px-5 text-xs ${ready ? 'text-slate-400' : 'text-rose-600'}">
          ${ready ? '-' : opsEscape(row.issue || '未满足自动发货条件')}
        </td>
      </tr>
    `;
  }).join('');
}

function opsAutoDeliverRenderScanResult(result) {
  const banner = document.getElementById('ops-ad-scan-result');
  if (!banner) return;
  const data = result || {};
  const summary = data.delivery_summary || {};
  const succeeded = Number(summary.succeeded);
  const failed = Number(summary.failed);
  const records = Number(summary.records);
  const errors = Array.isArray(data.errors) ? data.errors : [];

  banner.classList.remove('hidden');
  banner.innerHTML = `
    <div class="font-bold text-indigo-900">${opsEscape(data.message || '扫描完成')}</div>
    <div class="flex flex-wrap items-center gap-2 pt-0.5">
      <span class="px-2 py-0.5 rounded-full bg-emerald-100 text-emerald-700 text-[11px] font-semibold">成功 ${Number.isFinite(succeeded) ? succeeded : 0}</span>
      <span class="px-2 py-0.5 rounded-full bg-rose-100 text-rose-700 text-[11px] font-semibold">失败 ${Number.isFinite(failed) ? failed : 0}</span>
      <span class="px-2 py-0.5 rounded-full bg-indigo-100 text-indigo-700 text-[11px] font-semibold">记录 ${Number.isFinite(records) ? records : 0}</span>
    </div>
    ${errors.length
      ? `<div class="text-[11px] text-rose-600 break-all">${opsEscape(errors.slice(0, 3).join('；'))}</div>`
      : ''}
  `;
}

async function opsAutoDeliverLoadStats(showError) {
  try {
    const stats = await opsRequest('/api/auto-delivery/stats');
    opsAutoDeliverState.stats = stats || {};
    opsAutoDeliverRenderStats(opsAutoDeliverState.stats);
    return true;
  } catch (error) {
    if (opsHandleAuthError('ad', error)) return false;
    if (showError) opsToast(error.message || '自动发货统计加载失败', true);
    return false;
  }
}

async function opsAutoDeliverLoadReadiness(showError) {
  try {
    const data = await opsRequest('/api/auto-delivery/readiness');
    const rows = Array.isArray(data) ? data : (data && Array.isArray(data.items) ? data.items : []);
    opsAutoDeliverRenderReadiness(rows);
    return true;
  } catch (error) {
    if (opsHandleAuthError('ad', error)) return false;
    const tbody = document.getElementById('ops-ad-readiness-tbody');
    if (tbody) {
      tbody.innerHTML = `
        <tr>
          <td colspan="7" class="py-10 text-center text-xs text-rose-500">
            就绪检查加载失败：${opsEscape(error.message || '请稍后重试')}
            <button type="button" onclick="opsAutoDeliverRefresh()" class="ml-2 px-2.5 py-1 rounded bg-slate-900 text-white text-[11px]">重试</button>
          </td>
        </tr>
      `;
    }
    if (showError) opsToast(error.message || '就绪检查加载失败', true);
    return false;
  }
}

/** 自动发货页面入口（注册到 window.pageLoaders.autodeliver）。 */
async function loadOpsAutoDeliver() {
  if (!opsAutoDeliverEnsureSkeleton()) return;

  try {
    const results = await Promise.allSettled([
      opsRequest('/api/auto-delivery/stats'),
      opsRequest('/api/auto-delivery/config'),
      opsRequest('/api/auto-delivery/readiness')
    ]);

    const authFailed = results.some((result) => result.status === 'rejected' && opsIsAuthError(result.reason));
    if (authFailed) {
      opsSetAuthState('ad', true);
      return;
    }
    opsSetAuthState('ad', false);

    if (results[0].status === 'fulfilled') {
      opsAutoDeliverState.stats = results[0].value || {};
      opsAutoDeliverRenderStats(opsAutoDeliverState.stats);
    }

    if (results[1].status === 'fulfilled') {
      opsAutoDeliverFillConfig(results[1].value || {});
    } else {
      const stateEl = document.getElementById('ops-ad-config-state');
      if (stateEl) {
        stateEl.textContent = '加载失败';
        stateEl.className = 'shrink-0 text-[11px] px-2.5 py-1 rounded-full bg-rose-50 text-rose-600 font-semibold border border-rose-200';
      }
    }

    if (results[2].status === 'fulfilled') {
      const data = results[2].value;
      opsAutoDeliverRenderReadiness(Array.isArray(data) ? data : (data && Array.isArray(data.items) ? data.items : []));
    } else {
      const tbody = document.getElementById('ops-ad-readiness-tbody');
      if (tbody) {
        tbody.innerHTML = `<tr><td colspan="7" class="py-10 text-center text-xs text-rose-500">就绪检查加载失败：${opsEscape((results[2].reason && results[2].reason.message) || '请稍后重试')}</td></tr>`;
      }
    }

    const errors = results
      .filter((result) => result.status === 'rejected')
      .map((result) => (result.reason && result.reason.message) || '数据加载失败');
    if (errors.length > 0) opsToast(errors[0], true);
  } catch (error) {
    if (!opsHandleAuthError('ad', error)) opsToast(error.message || '自动发货页面加载失败', true);
  }
}

async function opsAutoDeliverRefresh() {
  await Promise.allSettled([
    opsAutoDeliverLoadStats(false),
    opsAutoDeliverLoadReadiness(false)
  ]);
}

async function opsAutoDeliverSave() {
  const saveButton = document.getElementById('ops-ad-save');
  if (saveButton && saveButton.disabled) return;

  const interval = Number(document.getElementById('ops-ad-scan-interval')?.value);
  const retryLimit = Number(document.getElementById('ops-ad-retry-limit')?.value);
  const lowStock = Number(document.getElementById('ops-ad-low-stock')?.value);

  if (!Number.isFinite(interval) || interval < 1) {
    opsToast('扫描间隔必须是大于 0 的秒数', true);
    return;
  }
  if (!Number.isFinite(retryLimit) || retryLimit < 0) {
    opsToast('失败重试次数不能小于 0', true);
    return;
  }
  if (!Number.isFinite(lowStock) || lowStock < 0) {
    opsToast('低库存阈值不能小于 0', true);
    return;
  }

  const payload = {
    enabled: !!(document.getElementById('ops-ad-enabled') && document.getElementById('ops-ad-enabled').checked),
    scan_interval_seconds: Math.trunc(interval),
    auto_consign: !!(document.getElementById('ops-ad-auto-consign') && document.getElementById('ops-ad-auto-consign').checked),
    auto_send_message: !!(document.getElementById('ops-ad-auto-send-message') && document.getElementById('ops-ad-auto-send-message').checked),
    reply_template: (document.getElementById('ops-ad-reply-template')?.value || '').trim(),
    retry_limit: Math.trunc(retryLimit),
    low_stock_threshold: Math.trunc(lowStock),
    notify_on_success: !!(document.getElementById('ops-ad-notify-success') && document.getElementById('ops-ad-notify-success').checked),
    notify_on_failure: !!(document.getElementById('ops-ad-notify-failure') && document.getElementById('ops-ad-notify-failure').checked),
    dry_run: !!(document.getElementById('ops-ad-dry-run') && document.getElementById('ops-ad-dry-run').checked)
  };

  if (saveButton) {
    saveButton.disabled = true;
    saveButton.textContent = '保存中...';
    saveButton.classList.add('opacity-60', 'cursor-not-allowed');
  }

  try {
    const saved = await opsRequest('/api/auto-delivery/config', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    });
    opsAutoDeliverFillConfig(Object.assign({}, payload, saved || {}));
    opsToast('配置已保存，扫描间隔已同步到定时任务');
    await opsAutoDeliverRefresh();
  } catch (error) {
    if (!opsHandleAuthError('ad', error)) opsToast(error.message || '配置保存失败', true);
  } finally {
    if (saveButton) {
      saveButton.disabled = false;
      saveButton.textContent = '💾 保存配置';
      saveButton.classList.remove('opacity-60', 'cursor-not-allowed');
    }
  }
}

async function opsAutoDeliverScan() {
  const state = opsAutoDeliverState;
  if (state.scanning) return;
  state.scanning = true;

  const scanButton = document.getElementById('ops-ad-scan');
  if (scanButton) {
    scanButton.disabled = true;
    scanButton.textContent = '扫描中...';
    scanButton.classList.add('opacity-60', 'cursor-not-allowed');
  }

  try {
    const result = await opsRequest('/api/auto-delivery/scan', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ account_id: opsCurrentAccountId() })
    });
    opsAutoDeliverRenderScanResult(result);
    opsToast((result && result.message) || '扫描完成');
    await opsAutoDeliverRefresh();
  } catch (error) {
    if (!opsHandleAuthError('ad', error)) opsToast(error.message || '自动发货扫描失败', true);
  } finally {
    state.scanning = false;
    if (scanButton) {
      scanButton.disabled = false;
      scanButton.textContent = '⚡ 立即扫描';
      scanButton.classList.remove('opacity-60', 'cursor-not-allowed');
    }
  }
}

/* =========================================================================
 * 四、发货记录（#view-deliveries）
 * ========================================================================= */

var opsDeliveriesState = {
  rendered: false,
  accounts: [],
  items: [],
  stats: null,
  total: 0,
  limit: 50,
  offset: 0,
  status: '',
  accountId: '',
  search: '',
  refreshSeq: 0,
  retrying: false
};

function opsDeliveriesEnsureSkeleton() {
  const root = document.getElementById('view-deliveries');
  if (!root) return false;
  if (opsDeliveriesState.rendered && root.querySelector('#ops-dlv-body')) return true;

  root.innerHTML = `
    <div class="space-y-4">
      ${opsAuthNoticeHtml('dlv', '登录后可查看每一笔自动 / 手动发货的审计流水并执行重试。')}

      <div id="ops-dlv-body" class="space-y-4">
        <div class="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3 sm:gap-4">
          <div class="bg-white rounded-xl p-4 border border-slate-200 shadow-sm">
            <div class="text-xs font-medium text-slate-500">📋 总记录</div>
            <div id="ops-dlv-stat-total" class="text-2xl font-bold text-slate-900 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">全部发货流水</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-emerald-200 shadow-sm">
            <div class="text-xs font-medium text-emerald-600">✅ 成功</div>
            <div id="ops-dlv-stat-success" class="text-2xl font-bold text-emerald-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">成功发货记录</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-rose-200 shadow-sm">
            <div class="text-xs font-medium text-rose-600">❌ 失败</div>
            <div id="ops-dlv-stat-failed" class="text-2xl font-bold text-rose-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">失败发货记录</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-indigo-200 shadow-sm">
            <div class="text-xs font-medium text-indigo-600">📅 今日</div>
            <div id="ops-dlv-stat-today" class="text-2xl font-bold text-indigo-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">今天产生的记录</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-emerald-200 shadow-sm">
            <div class="text-xs font-medium text-emerald-600">📈 成功率</div>
            <div id="ops-dlv-stat-rate" class="text-2xl font-bold text-emerald-600 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">成功 / 全部记录</div>
          </div>
          <div class="bg-white rounded-xl p-4 border border-slate-200 shadow-sm">
            <div class="text-xs font-medium text-slate-500">⏱️ 平均耗时</div>
            <div id="ops-dlv-stat-duration" class="text-2xl font-bold text-slate-900 mt-1">-</div>
            <div class="text-[11px] text-slate-400 mt-0.5">单次发货平均耗时</div>
          </div>
        </div>

        <div class="bg-white rounded-xl border border-slate-200 shadow-sm p-4">
          <div class="flex flex-wrap items-end gap-3">
            <div>
              <label class="block text-[11px] font-semibold text-slate-500 mb-1">状态</label>
              <select id="ops-dlv-status" onchange="opsDeliveriesStatusChanged(this.value)"
                class="rounded-lg border border-slate-300 bg-white px-3 py-2 text-xs font-semibold text-slate-700 focus:outline-none focus:border-indigo-500">
                <option value="">全部状态</option>
                <option value="success">✅ 成功</option>
                <option value="failed">❌ 失败</option>
                <option value="pending">⏳ 处理中</option>
              </select>
            </div>
            <div>
              <label class="block text-[11px] font-semibold text-slate-500 mb-1">闲鱼账号</label>
              <select id="ops-dlv-account" onchange="opsDeliveriesAccountChanged(this.value)"
                class="min-w-[190px] rounded-lg border border-slate-300 bg-white px-3 py-2 text-xs font-semibold text-slate-700 focus:outline-none focus:border-indigo-500">
                <option value="">全部账号</option>
              </select>
            </div>
            <div class="flex-1 min-w-[240px]">
              <label class="block text-[11px] font-semibold text-slate-500 mb-1">搜索</label>
              <input id="ops-dlv-search" type="text" placeholder="平台单号 / 买家 / 商品 / 卡密"
                onkeydown="opsDeliveriesSearchKeydown(event)"
                class="w-full rounded-lg border border-slate-300 px-3 py-2 text-xs" />
            </div>
            <div class="flex flex-wrap items-center gap-2">
              <button type="button" onclick="opsDeliveriesApplyFilters()"
                class="px-3.5 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-700 text-white text-xs font-semibold shadow-xs">
                🔍 筛选
              </button>
              <button type="button" onclick="opsDeliveriesRefresh()"
                class="px-3.5 py-2 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-semibold border border-slate-300 shadow-xs">
                刷新
              </button>
              <button type="button" id="ops-dlv-export" onclick="opsDeliveriesExport()"
                class="px-3.5 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-700 text-white text-xs font-semibold shadow-xs">
                ⬇️ 导出 CSV
              </button>
            </div>
          </div>
        </div>

        <div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">
          <div class="px-5 py-4 border-b border-slate-200 flex flex-wrap items-center justify-between gap-2">
            <div>
              <h2 class="text-base font-bold text-slate-900">发货记录</h2>
              <p class="text-xs text-slate-500 mt-0.5">每次自动 / 手动发货的完整审计流水，可查看详情、重试或删除</p>
            </div>
            <span id="ops-dlv-count-label" class="text-[11px] text-slate-400">共 0 条</span>
          </div>

          <div class="overflow-x-auto">
            <table class="w-full text-left border-collapse min-w-[1320px]">
              <thead>
                <tr class="bg-slate-50 border-b border-slate-200 text-xs font-semibold text-slate-600">
                  <th class="py-3 px-4">ID</th>
                  <th class="py-3 px-3">平台单号</th>
                  <th class="py-3 px-3">商品</th>
                  <th class="py-3 px-3">买家</th>
                  <th class="py-3 px-3">状态</th>
                  <th class="py-3 px-3">来源</th>
                  <th class="py-3 px-3">发货内容</th>
                  <th class="py-3 px-3">远端 consign / IM</th>
                  <th class="py-3 px-3">耗时</th>
                  <th class="py-3 px-4">时间</th>
                  <th class="py-3 px-4 text-right">操作</th>
                </tr>
              </thead>
              <tbody id="ops-dlv-tbody" class="divide-y divide-slate-100 text-sm">
                <tr><td colspan="11" class="py-10 text-center text-xs text-slate-400">正在加载发货记录...</td></tr>
              </tbody>
            </table>
          </div>

          <div class="px-5 py-3 border-t border-slate-200 flex flex-wrap items-center justify-between gap-3">
            <span id="ops-dlv-page-info" class="text-xs text-slate-500">共 0 条 · 每页 50 条</span>
            <div class="flex items-center gap-2">
              <button type="button" id="ops-dlv-prev" onclick="opsDeliveriesPage(-1)"
                class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-semibold border border-slate-300">
                上一页
              </button>
              <span id="ops-dlv-page-num" class="text-xs text-slate-500 min-w-[86px] text-center">第 1 / 1 页</span>
              <button type="button" id="ops-dlv-next" onclick="opsDeliveriesPage(1)"
                class="px-3 py-1.5 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-semibold border border-slate-300">
                下一页
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  `;

  opsDeliveriesState.rendered = true;
  opsSetValue('ops-dlv-status', opsDeliveriesState.status);
  opsSetValue('ops-dlv-search', opsDeliveriesState.search);
  return true;
}

function opsDeliveriesStatusText(status) {
  const labels = {
    success: '成功',
    failed: '失败',
    pending: '处理中'
  };
  return labels[status] || status || '未知';
}

function opsDeliveriesStatusBadge(status) {
  if (status === 'success') {
    return '<span class="inline-flex items-center px-2 py-0.5 rounded-full bg-emerald-50 text-emerald-700 border border-emerald-200 text-[11px] font-semibold">✅ 成功</span>';
  }
  if (status === 'failed') {
    return '<span class="inline-flex items-center px-2 py-0.5 rounded-full bg-rose-50 text-rose-700 border border-rose-200 text-[11px] font-semibold">❌ 失败</span>';
  }
  if (status === 'pending') {
    return '<span class="inline-flex items-center px-2 py-0.5 rounded-full bg-amber-50 text-amber-700 border border-amber-200 text-[11px] font-semibold">⏳ 处理中</span>';
  }
  return `<span class="inline-flex items-center px-2 py-0.5 rounded-full bg-slate-100 text-slate-600 border border-slate-200 text-[11px] font-semibold">${opsEscape(status || '未知')}</span>`;
}

function opsDeliveriesSourceLabel(source) {
  const labels = {
    auto_scan: '自动扫描',
    webhook: 'Webhook',
    manual: '手动',
    api: 'API',
    retry: '重试'
  };
  return labels[source] || source || '-';
}

function opsDeliveriesRemoteStatus(value) {
  if (value === null || value === undefined || value === '') return '<span class="text-slate-400">-</span>';
  const text = String(value);
  const ok = /^(success|ok|sent|delivered)\b/i.test(text) || /已发货|已发送/.test(text);
  return `<span class="font-mono text-[10px] ${ok ? 'text-emerald-600' : 'text-rose-600'}">${opsEscape(text)}</span>`;
}

function opsDeliveriesRenderStats(stats) {
  const data = stats || {};
  opsSetText('ops-dlv-stat-total', opsCount(data.total));
  opsSetText('ops-dlv-stat-success', opsCount(data.success));
  opsSetText('ops-dlv-stat-failed', opsCount(data.failed));
  opsSetText('ops-dlv-stat-today', opsCount(data.today));
  opsSetText('ops-dlv-stat-rate', opsPercent(data.success_rate));
  opsSetText('ops-dlv-stat-duration', opsFormatDuration(data.avg_duration_ms));

  const badge = document.getElementById('badge-deliveries');
  if (badge) badge.textContent = opsCount(data.total);
}

function opsDeliveriesRenderTable() {
  const tbody = document.getElementById('ops-dlv-tbody');
  if (!tbody) return;
  const state = opsDeliveriesState;
  const rows = Array.isArray(state.items) ? state.items : [];

  opsSetText('ops-dlv-count-label', `共 ${state.total} 条`);

  if (rows.length === 0) {
    const filtered = !!(state.status || state.accountId || state.search);
    tbody.innerHTML = `
      <tr>
        <td colspan="11" class="py-14 text-center">
          <div class="text-3xl mb-2">${filtered ? '🔍' : '📭'}</div>
          <div class="text-xs font-medium text-slate-500">${filtered ? '没有符合筛选条件的发货记录' : '暂无发货记录'}</div>
          <div class="text-[11px] text-slate-400 mt-1">${filtered ? '试试清空筛选条件或搜索其他关键词' : '自动发货或手动发货后，审计流水会显示在这里'}</div>
        </td>
      </tr>
    `;
    return;
  }

  tbody.innerHTML = rows.map((record) => {
    const row = record || {};
    const id = opsSafeInt(row.id);
    const content = row.delivery_content;
    const contentCell = content
      ? `<button type="button" onclick="opsShowDeliveryDetail(${id})" title="${opsEscape(content)}"
           class="block max-w-[210px] truncate text-left font-mono text-[11px] text-indigo-700 hover:underline">${opsEscape(content)}</button>`
      : '<span class="text-slate-400">-</span>';

    return `
      <tr class="hover:bg-slate-50/80 transition">
        <td class="py-3 px-4 font-mono text-xs text-slate-600">#${id}</td>
        <td class="py-3 px-3">
          <div class="font-mono text-[11px] text-slate-800 max-w-[170px] truncate" title="${opsEscape(row.platform_order_id || '')}">${opsEscape(row.platform_order_id || '-')}</div>
          ${row.order_id ? `<div class="text-[10px] text-slate-400 font-mono">订单 #${opsSafeInt(row.order_id)}</div>` : ''}
        </td>
        <td class="py-3 px-3">
          <div class="text-xs font-semibold text-slate-800 max-w-[180px] truncate" title="${opsEscape(row.product_name || '')}">${opsEscape(row.product_name || '-')}</div>
          <div class="text-[10px] text-slate-400 font-mono">商品 #${row.product_id ? opsSafeInt(row.product_id) : '-'}</div>
        </td>
        <td class="py-3 px-3 text-xs text-slate-700 max-w-[140px] truncate" title="${opsEscape(row.buyer_nickname || '')}">${opsEscape(row.buyer_nickname || '-')}</td>
        <td class="py-3 px-3">${opsDeliveriesStatusBadge(row.status)}</td>
        <td class="py-3 px-3">
          <span class="px-2 py-0.5 rounded bg-slate-100 text-slate-600 text-[11px]">${opsEscape(opsDeliveriesSourceLabel(row.source))}</span>
        </td>
        <td class="py-3 px-3">
          ${contentCell}
          ${row.error_message ? '<div class="text-[10px] text-rose-500 mt-0.5" title="' + opsEscape(row.error_message) + '">⚠ 有错误信息</div>' : ''}
        </td>
        <td class="py-3 px-3 space-y-0.5">
          <div class="text-[10px] text-slate-400">发货：${opsDeliveriesRemoteStatus(row.remote_consign_status)}</div>
          <div class="text-[10px] text-slate-400">IM：${opsDeliveriesRemoteStatus(row.remote_im_status)}</div>
        </td>
        <td class="py-3 px-3 text-[11px] text-slate-500 whitespace-nowrap">${opsEscape(opsFormatDuration(row.duration_ms))}</td>
        <td class="py-3 px-4 text-[11px] text-slate-400 whitespace-nowrap">${opsEscape(opsFmtTime(row.created_at))}</td>
        <td class="py-3 px-4 text-right whitespace-nowrap space-x-1">
          <button type="button" onclick="opsShowDeliveryDetail(${id})"
            class="px-2 py-1 rounded-lg bg-indigo-50 hover:bg-indigo-100 text-indigo-700 text-[11px] font-medium">详情</button>
          <button type="button" onclick="opsRetryDelivery(${id})"
            class="px-2 py-1 rounded-lg bg-emerald-50 hover:bg-emerald-100 text-emerald-700 text-[11px] font-medium">重试</button>
          <button type="button" onclick="opsDeleteDelivery(${id})"
            class="px-2 py-1 rounded-lg bg-rose-50 hover:bg-rose-100 text-rose-600 text-[11px] font-medium">删除</button>
        </td>
      </tr>
    `;
  }).join('');
}

function opsDeliveriesUpdatePager() {
  const state = opsDeliveriesState;
  const page = Math.floor(state.offset / state.limit) + 1;
  const totalPages = Math.max(1, Math.ceil(state.total / state.limit));

  opsSetText('ops-dlv-page-info', `共 ${state.total} 条 · 每页 ${state.limit} 条`);
  opsSetText('ops-dlv-page-num', `第 ${page} / ${totalPages} 页`);

  [
    ['ops-dlv-prev', state.offset <= 0],
    ['ops-dlv-next', state.total === 0 || state.offset + state.limit >= state.total]
  ].forEach(([buttonId, disabled]) => {
    const button = document.getElementById(buttonId);
    if (!button) return;
    button.disabled = !!disabled;
    button.classList.toggle('opacity-40', !!disabled);
    button.classList.toggle('cursor-not-allowed', !!disabled);
  });
}

function opsDeliveriesBuildQuery() {
  const state = opsDeliveriesState;
  const params = new URLSearchParams();
  if (state.status) params.set('status', state.status);
  if (state.accountId) params.set('account_id', state.accountId);
  if (state.search) params.set('search', state.search);
  params.set('limit', String(state.limit));
  params.set('offset', String(state.offset));
  return params.toString();
}

async function opsDeliveriesLoadAccounts() {
  const accounts = await opsLoadXianyuAccounts();
  opsDeliveriesState.accounts = accounts;
  const selected = opsFillAccountSelect('ops-dlv-account', accounts, opsDeliveriesState.accountId, '全部账号');
  opsDeliveriesState.accountId = selected;
}

function opsDeliveriesReadFilterInputs() {
  const state = opsDeliveriesState;
  const statusEl = document.getElementById('ops-dlv-status');
  const accountEl = document.getElementById('ops-dlv-account');
  const searchEl = document.getElementById('ops-dlv-search');
  if (statusEl) state.status = statusEl.value || '';
  if (accountEl) state.accountId = accountEl.value || '';
  if (searchEl) state.search = searchEl.value.trim();
}

function opsDeliveriesStatusChanged(value) {
  opsDeliveriesState.status = value || '';
  opsDeliveriesReadFilterInputs();
  opsDeliveriesState.offset = 0;
  opsDeliveriesRefresh();
}

function opsDeliveriesAccountChanged(value) {
  opsDeliveriesState.accountId = value || '';
  opsDeliveriesReadFilterInputs();
  opsDeliveriesState.offset = 0;
  opsDeliveriesRefresh();
}

function opsDeliveriesSearchKeydown(event) {
  if (event && event.key === 'Enter' && !event.isComposing) {
    event.preventDefault();
    opsDeliveriesApplyFilters();
  }
}

function opsDeliveriesApplyFilters() {
  opsDeliveriesReadFilterInputs();
  opsDeliveriesState.offset = 0;
  return opsDeliveriesRefresh();
}

async function opsDeliveriesLoadData(options) {
  const opts = options || {};
  const state = opsDeliveriesState;
  if (!document.getElementById('ops-dlv-tbody')) return;

  const seq = ++state.refreshSeq;
  const query = opsDeliveriesBuildQuery();
  const results = await Promise.allSettled([
    opsRequest('/api/deliveries/stats'),
    opsRequest(`/api/deliveries?${query}`)
  ]);
  if (seq !== state.refreshSeq) return;

  const authFailed = results.some((result) => result.status === 'rejected' && opsIsAuthError(result.reason));
  if (authFailed) {
    opsSetAuthState('dlv', true);
    return;
  }
  opsSetAuthState('dlv', false);

  if (results[0].status === 'fulfilled') {
    state.stats = results[0].value || {};
    opsDeliveriesRenderStats(state.stats);
  } else if (!opts.silent) {
    opsToast((results[0].reason && results[0].reason.message) || '发货统计加载失败', true);
  }

  if (results[1].status === 'rejected') {
    if (!opts.silent) opsToast((results[1].reason && results[1].reason.message) || '发货记录加载失败', true);
    const tbody = document.getElementById('ops-dlv-tbody');
    if (tbody) {
      tbody.innerHTML = `
        <tr>
          <td colspan="11" class="py-12 text-center">
            <div class="text-xs font-medium text-rose-600">发货记录加载失败</div>
            <div class="text-[11px] text-slate-400 mt-1 break-all">${opsEscape((results[1].reason && results[1].reason.message) || '请稍后重试')}</div>
            <button type="button" onclick="opsDeliveriesRefresh()"
              class="mt-3 px-3 py-1.5 rounded-lg bg-slate-900 text-white text-xs font-semibold">重试</button>
          </td>
        </tr>
      `;
    }
    return;
  }

  const data = results[1].value;
  const items = Array.isArray(data) ? data : (data && Array.isArray(data.items) ? data.items : []);
  const totalValue = data && Number(data.total);
  const total = Number.isFinite(totalValue) ? totalValue : items.length;

  state.items = items;
  state.total = total;

  // 删除最后一条 / 筛选后页码越界时，自动回退到最后一个有数据的区间。
  if (items.length === 0 && total > 0 && state.offset > 0) {
    state.offset = Math.max(0, Math.floor((total - 1) / state.limit) * state.limit);
    return opsDeliveriesLoadData({ silent: true });
  }

  opsDeliveriesRenderTable();
  opsDeliveriesUpdatePager();
}

function opsDeliveriesRefresh(options) {
  return opsDeliveriesLoadData(options).catch((error) => {
    if (!opsHandleAuthError('dlv', error)) opsToast(error.message || '发货记录刷新失败', true);
  });
}

function opsDeliveriesPage(delta) {
  const state = opsDeliveriesState;
  const nextOffset = state.offset + Number(delta) * state.limit;
  if (!Number.isFinite(nextOffset) || nextOffset < 0) return;
  if (Number(delta) > 0 && nextOffset >= state.total) return;
  state.offset = nextOffset;
  opsDeliveriesRefresh();
}

async function opsShowDeliveryDetail(id) {
  const recordId = opsSafeInt(id);
  const row = opsDeliveriesState.items.find((item) => Number(item.id) === recordId);
  if (!row) {
    opsToast('未找到该发货记录，请刷新后重试', true);
    return;
  }

  const existing = document.getElementById('ops-delivery-modal');
  if (existing) existing.remove();

  const fields = [
    ['记录 ID', `#${recordId}`],
    ['订单 ID', row.order_id ? `#${opsSafeInt(row.order_id)}` : '-'],
    ['平台单号', row.platform_order_id || '-'],
    ['商品', row.product_name || '-'],
    ['商品 ID', row.product_id ? `#${opsSafeInt(row.product_id)}` : '-'],
    ['闲鱼宝贝 ID', row.xianyu_item_id || '-'],
    ['买家', row.buyer_nickname || '-'],
    ['账号 ID', row.account_id ? `#${opsSafeInt(row.account_id)}` : '-'],
    ['状态', opsDeliveriesStatusText(row.status)],
    ['来源', opsDeliveriesSourceLabel(row.source)],
    ['远端发货状态', row.remote_consign_status || '-'],
    ['远端 IM 状态', row.remote_im_status || '-'],
    ['耗时', opsFormatDuration(row.duration_ms)],
    ['创建时间', opsFmtTime(row.created_at)]
  ];
  if (row.amount !== null && row.amount !== undefined) {
    fields.push(['金额', `¥${opsYuan(row.amount)}`]);
  }

  const fieldHtml = fields.map(([label, value]) => `
    <div class="rounded-lg bg-slate-50 border border-slate-200 px-3 py-2 min-w-0">
      <div class="text-[10px] text-slate-400">${opsEscape(label)}</div>
      <div class="font-medium text-slate-800 break-all mt-0.5 text-xs">${opsEscape(value)}</div>
    </div>
  `).join('');

  const errorMessage = row.error_message
    ? `<pre class="mt-1.5 whitespace-pre-wrap break-all rounded-lg border border-rose-200 bg-rose-50 px-3 py-2 text-[11px] text-rose-700">${opsEscape(row.error_message)}</pre>`
    : '<div class="mt-1.5 rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 text-[11px] text-slate-400">无错误信息</div>';

  const modalHtml = `
    <div id="ops-delivery-modal" class="fixed inset-0 bg-slate-900/50 z-50 flex items-center justify-center p-4"
      onclick="if (event.target === this) opsCloseDeliveryModal()">
      <div class="bg-white rounded-2xl border border-slate-200 shadow-xl max-w-2xl w-full max-h-[90vh] flex flex-col overflow-hidden">
        <div class="px-6 py-4 border-b border-slate-200 flex items-start justify-between gap-3 shrink-0">
          <div>
            <h3 class="text-base font-bold text-slate-900">发货记录 #${recordId}</h3>
            <p class="text-[11px] text-slate-500 mt-0.5">${opsEscape(opsFmtTime(row.created_at))} · ${opsEscape(opsDeliveriesSourceLabel(row.source))}</p>
          </div>
          <button type="button" onclick="opsCloseDeliveryModal()"
            class="text-slate-400 hover:text-slate-700 text-lg leading-none px-1">✕</button>
        </div>

        <div class="p-6 space-y-4 overflow-y-auto">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">${fieldHtml}</div>

          <div>
            <div class="text-xs font-bold text-slate-700">发货内容</div>
            <pre class="mt-1.5 whitespace-pre-wrap break-all rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 text-[11px] text-slate-700 font-mono">${opsEscape(row.delivery_content || '-')}</pre>
          </div>

          <div>
            <div class="text-xs font-bold text-slate-700">错误信息</div>
            ${errorMessage}
          </div>
        </div>

        <div class="px-6 py-3 border-t border-slate-200 flex items-center justify-end gap-2 shrink-0">
          ${row.status !== 'success'
            ? `<button type="button" onclick="opsRetryDelivery(${recordId})"
                 class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-700 text-white text-xs font-semibold">重试发货</button>`
            : ''}
          <button type="button" onclick="opsCloseDeliveryModal()"
            class="px-4 py-2 rounded-lg bg-slate-100 hover:bg-slate-200 text-slate-700 text-xs font-semibold border border-slate-300">关闭</button>
        </div>
      </div>
    </div>
  `;

  document.body.insertAdjacentHTML('beforeend', modalHtml);
}

function opsCloseDeliveryModal() {
  const modal = document.getElementById('ops-delivery-modal');
  if (modal) modal.remove();
}

async function opsRetryDelivery(id) {
  const recordId = opsSafeInt(id);
  if (recordId <= 0) return;
  if (opsDeliveriesState.retrying) return;
  if (!window.confirm(`确认重试发货记录 #${recordId}？将按原平台单号重新执行发货。`)) return;

  opsDeliveriesState.retrying = true;
  try {
    const result = await opsRequest(`/api/deliveries/${recordId}/retry`, { method: 'POST' });
    opsToast((result && result.message) || `发货记录 #${recordId} 重试完成`);
    opsCloseDeliveryModal();
    await opsDeliveriesRefresh({ silent: true });
  } catch (error) {
    if (!opsHandleAuthError('dlv', error)) opsToast(error.message || '重试发货失败', true);
  } finally {
    opsDeliveriesState.retrying = false;
  }
}

async function opsDeleteDelivery(id) {
  const recordId = opsSafeInt(id);
  if (recordId <= 0) return;
  if (!window.confirm(`确认删除发货记录 #${recordId}？该操作不可撤销。`)) return;

  try {
    await opsRequest(`/api/deliveries/${recordId}`, { method: 'DELETE' });
    opsToast(`发货记录 #${recordId} 已删除`);
    opsCloseDeliveryModal();
    await opsDeliveriesRefresh({ silent: true });
  } catch (error) {
    if (!opsHandleAuthError('dlv', error)) opsToast(error.message || '删除发货记录失败', true);
  }
}

async function opsDeliveriesExport() {
  const button = document.getElementById('ops-dlv-export');
  if (button && button.disabled) return;
  if (button) {
    button.disabled = true;
    button.textContent = '导出中...';
    button.classList.add('opacity-60', 'cursor-not-allowed');
  }

  try {
    const headers = {};
    let token = '';
    try {
      token = localStorage.getItem('adm_token') || '';
    } catch (_) {
      token = '';
    }
    if (token) headers.Authorization = `Bearer ${token}`;

    // 导出接口返回 CSV，必须用 fetch 带 token 获取 blob，不能直接 window.open。
    const response = await fetch('/api/deliveries/export', { headers });
    if (response.status === 401) {
      const authError = new Error('请先登录管理员账号');
      authError.isAuth = true;
      opsHandleAuthError('dlv', authError);
      opsToast('请先登录管理员账号', true);
      return;
    }
    if (!response.ok) {
      let message = `导出失败（HTTP ${response.status}）`;
      try {
        const text = await response.text();
        if (text) {
          try {
            const data = JSON.parse(text);
            message = (data && data.message) || message;
          } catch (_) {
            message = text.slice(0, 200) || message;
          }
        }
      } catch (_) {
        /* ignore */
      }
      throw new Error(message);
    }

    const blob = await response.blob();
    const objectUrl = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = objectUrl;
    link.download = 'deliveries.csv';
    link.style.display = 'none';
    document.body.appendChild(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(objectUrl), 2000);
    opsToast('发货记录 CSV 已开始下载');
  } catch (error) {
    if (!opsHandleAuthError('dlv', error)) opsToast(error.message || '导出失败', true);
  } finally {
    if (button) {
      button.disabled = false;
      button.textContent = '⬇️ 导出 CSV';
      button.classList.remove('opacity-60', 'cursor-not-allowed');
    }
  }
}

/** 发货记录页面入口（注册到 window.pageLoaders.deliveries）。 */
async function loadOpsDeliveries() {
  if (!opsDeliveriesEnsureSkeleton()) return;

  try {
    try {
      await opsDeliveriesLoadAccounts();
    } catch (error) {
      if (opsHandleAuthError('dlv', error)) return;
    }

    await opsDeliveriesRefresh({ silent: false });
  } catch (error) {
    if (!opsHandleAuthError('dlv', error)) opsToast(error.message || '发货记录页面加载失败', true);
  }
}

/* =========================================================================
 * 五、页面加载器注册（必须放在最后，顶层不得立即发起请求）
 * ========================================================================= */

window.pageLoaders = window.pageLoaders || {};
window.pageLoaders['messages'] = loadOpsMessages;
window.pageLoaders['autodeliver'] = loadOpsAutoDeliver;
window.pageLoaders['deliveries'] = loadOpsDeliveries;
