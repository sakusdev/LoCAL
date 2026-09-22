'use strict';

const $ = (id) => document.getElementById(id);
const escapeHtml = (value) => String(value ?? '').replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const android = Boolean(window.LocalNative);
const desktop = Boolean(window.__TAURI__);
let snapshot = null;
let activeTab = 'nearby';
let chosenPeer = '';
let nativeCounter = 0;
let toastTimer;
let messageSignature = '';
const pending = new Map();
const renderCache = new Map();
const busy = new Set();
let receivedFiles = [];
let receivedOffset = 0;
let receivedSignature = null;
let receivedLoading = false;

window.localResolve = (id, raw) => {
  const handler = pending.get(id);
  if (!handler) return;
  pending.delete(id);
  clearTimeout(handler.timer);
  try { const reply = JSON.parse(raw); if (reply.ok) handler.resolve(reply.data); else handler.reject(new Error(reply.error)); }
  catch (error) { handler.reject(error); }
};

async function command(request) {
  if (android) {
    const id = String(++nativeCounter);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { pending.delete(id); reject(new Error('応答がありません。接続を確認してください。')); }, ['pick_file','export_file'].includes(request.op) ? 30 * 60 * 1000 : 150000);
      pending.set(id, { resolve, reject, timer });
      window.LocalNative.invoke(id, JSON.stringify(request));
    });
  }
  if (desktop) {
    const invoke = window.__TAURI__.core.invoke;
    if (request.op === 'pick_file') {
      const path = await invoke('plugin:dialog|open', { options: { multiple: false, directory: false, title: '送信するファイルを選択' } });
      if (!path) return null;
      return invoke('local_command', { request: { op:'send_file', peer_id:request.peer_id, path } });
    }
    if (request.op === 'read_clipboard') return invoke('plugin:clipboard-manager|read_text');
    if (request.op === 'write_clipboard') return invoke('plugin:clipboard-manager|write_text', { text: request.text });
    return invoke('local_command', { request });
  }
  throw new Error('Android / デスクトップ版LoCALから開いてください。ブラウザーだけではLANに接続できません。');
}

function toast(message, error = false) {
  clearTimeout(toastTimer);
  $('toast').textContent = message;
  $('toast').className = error ? 'error' : '';
  $('toast').hidden = false;
  toastTimer = setTimeout(() => { $('toast').hidden = true; }, error ? 7000 : 3600);
}

async function action(key, work) {
  if (busy.has(key)) return;
  busy.add(key);
  try { await work(); await refresh(); }
  catch (error) { toast(String(error.message || error), true); }
  finally { busy.delete(key); }
}

function setHtml(id, html) {
  if (renderCache.get(id) === html) return;
  const focus = document.activeElement;
  const focusAction = focus?.dataset.action;
  const focusId = focus?.dataset.id;
  $(id).innerHTML = html;
  renderCache.set(id, html);
  if (focusAction && focusId) {
    [...$(id).querySelectorAll('[data-action]')].find((el) => el.dataset.action === focusAction && el.dataset.id === focusId)?.focus({ preventScroll: true });
  }
}

function tab(name) {
  activeTab = name;
  const labels = { nearby:'近くの端末', transfers:'ファイル転送', history:'メッセージ', screen:'画面共有', settings:'設定' };
  document.querySelectorAll('.view').forEach((view) => { view.hidden = view.id !== `view-${name}`; });
  document.querySelectorAll('[data-tab]').forEach((button) => { button.classList.toggle('active', button.dataset.tab === name); button.setAttribute('aria-current', button.dataset.tab === name ? 'page' : 'false'); });
  $('page-name').textContent = labels[name];
  render();
  if (name === 'transfers') refreshReceived(true);
}

function size(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), 3);
  return `${(bytes / 1024 ** index).toFixed(1)} ${['B','KiB','MiB','GiB'][index]}`;
}
const empty = (title, body) => `<div class="empty"><span class="empty-symbol">⌁</span><strong>${escapeHtml(title)}</strong><p>${escapeHtml(body)}</p></div>`;
const button = (label, actionName, id, className = 'secondary') => `<button class="${className}" data-action="${actionName}" data-id="${escapeHtml(id)}">${escapeHtml(label)}</button>`;
const peerName = (id) => snapshot?.peers.find((p) => p.id === id)?.name || snapshot?.trusted.find((p) => p.id === id)?.name || id.slice(0, 10);

function render() {
  if (!snapshot) return;
  $('self-name').textContent = snapshot.device.name;
  $('self-address').textContent = snapshot.device.addresses[0] || `Port ${snapshot.device.port}`;
  const ready = snapshot.peers.filter((p) => p.ready);
  $('connection').lastElementChild.textContent = ready.length ? `${ready.length} 台と接続中` : 'LANで待機中';
  $('peer-count').textContent = snapshot.peers.length;
  $('nearby-count').textContent = snapshot.peers.length;
  $('transfer-count').textContent = snapshot.transfers.filter((t) => !['completed','failed','cancelled'].includes(t.status)).length;
  $('version').textContent = snapshot.version;
  $('warnings').hidden = !snapshot.warnings.length;
  setHtml('warnings', snapshot.warnings.length ? `<div class="notice">${snapshot.warnings.map(escapeHtml).join('<br>')}</div>` : '');
  if (activeTab === 'nearby') renderPeers();
  if (activeTab === 'transfers') renderTransfers();
  if (activeTab === 'history') renderMessages();
  if (activeTab === 'settings') renderSettings();
}

function renderPeers() {
  setHtml('peers', snapshot.peers.length ? snapshot.peers.map((peer) => {
    let controls;
    if (peer.ready) {
      controls = `<div class="peer-status">● ペアリング済み · 暗号化接続</div><div class="actions">${button('ファイルを送る ↗','file',peer.id,'primary')}${button('メッセージ','chat',peer.id)}${button('切断','disconnect',peer.id)}</div>`;
    } else if (peer.connected && peer.code) {
      controls = `<div class="pair-box"><p>相手の画面と同じコードですか？</p><div class="pair-code">${escapeHtml(peer.code)}</div>${peer.local_confirmed ? '<p>相手の確認を待っています…</p>' : button('一致しています','pair',peer.id,'primary full')}<small>相手の画面を直接確認してください。コードは2分で失効します。</small></div>${button('接続を取り消す','disconnect',peer.id)}`;
    } else controls = `<div class="peer-status">${peer.trusted ? '◇ 信頼済みの端末' : '○ 接続できます'}</div>${button(peer.trusted ? 'もう一度つなぐ ↗' : 'ペアリングする ↗','connect',peer.id,'primary full')}`;
    return `<article class="peer-card ${peer.ready ? 'ready' : ''}"><div class="peer-heading"><div class="peer-device" aria-hidden="true">▣</div><div><h3>${escapeHtml(peer.name)}</h3><div class="mono">${escapeHtml(peer.address)}</div></div></div><div class="peer-controls">${controls}</div></article>`;
  }).join('') : empty('端末を探しています', '同じWi-Fiにつないで、もう一方の端末でもLoCALを開いてください。'));
}

function renderTransfers() {
  const statuses = { hashing:'送信の準備中', awaiting_acceptance:'相手の承認待ち', offered:'受信の確認', transferring:'転送中', verifying:'内容を検証中', completed:'完了', failed:'転送できませんでした', cancelled:'キャンセル済み' };
  setHtml('transfers', snapshot.transfers.length ? snapshot.transfers.map((transfer) => {
    const active = !['completed','failed','cancelled'].includes(transfer.status);
    let controls = '';
    if (transfer.status === 'offered') controls = button('受信する','accept',transfer.id,'primary') + button('辞退','reject',transfer.id);
    else if (active) controls = button('キャンセル','cancel',transfer.id);
    if (transfer.status === 'completed' && transfer.direction === 'in' && transfer.path) controls += button(android ? '端末に保存…' : '保存先をコピー','export',transfer.id,'primary');
    if (['failed','cancelled'].includes(transfer.status) && transfer.direction === 'out') controls += button('再送する','retry',transfer.id);
    return `<article class="transfer"><div class="transfer-header"><div><h3>${escapeHtml(transfer.name)}</h3><div class="transfer-meta">${transfer.direction === 'in' ? '↓ 受信' : '↑ 送信'} · ${escapeHtml(peerName(transfer.peer_id))} · ${size(transfer.bytes)} / ${size(transfer.size)}</div></div><span class="transfer-status ${transfer.status === 'failed' ? 'failed' : ''}">${statuses[transfer.status] || escapeHtml(transfer.status)}</span></div>${active ? `<progress value="${transfer.bytes}" max="${transfer.size || 1}" aria-label="転送の進行状況"></progress>` : ''}${transfer.error ? `<p class="error-text">${escapeHtml(transfer.error)}</p>` : ''}${controls ? `<div class="actions">${controls}</div>` : ''}</article>`;
  }).join('') : empty('まだ転送はありません', '「近くの端末」から相手を選んで、最初のファイルを送ってみましょう。'));
  refreshReceived();
}

async function exportReceived(file) {
  if (android) {
    const result = await command({op:'export_file',path:file.path,name:file.name});
    if (result) toast('ファイルを保存しました');
  } else {
    await command({op:'write_clipboard',text:file.path});
    toast('保存先をコピーしました。ファイルマネージャーで開けます。');
  }
}

async function refreshReceived(force = false) {
  if (receivedLoading || (!android && !desktop)) return;
  const signature = snapshot?.transfers.filter(t => t.direction === 'in' && t.path).map(t => `${t.id}:${t.status}`).join(',') || '';
  if (!force && signature === receivedSignature) return;
  receivedLoading = true;
  $('received-prev').disabled = true;
  $('received-next').disabled = true;
  try {
    const result = await command({op:'received_files',offset:receivedOffset});
    receivedFiles = result.files;
    receivedOffset = result.offset;
    receivedSignature = signature;
    setHtml('received-files', receivedFiles.length ? receivedFiles.map((file, index) => `<article class="transfer"><h3>${escapeHtml(file.name)}</h3><div class="transfer-meta">${size(file.size)} · ${new Date(file.timestamp).toLocaleString('ja-JP')}</div><div class="actions">${button(android ? '端末に保存…' : '保存先をコピー','export-received',String(index),'primary')}</div></article>`).join('') : empty('保存済みのファイルはありません','受信が完了したファイルをここに表示します。'));
    $('received-page').textContent = result.total ? `${receivedOffset + 1}–${receivedOffset + receivedFiles.length} / ${result.total} 件` : '0 件';
    $('received-prev').disabled = receivedOffset === 0;
    $('received-next').disabled = receivedOffset + receivedFiles.length >= result.total;
  } catch (error) {
    receivedSignature = signature;
    toast(String(error.message || error),true);
  } finally { receivedLoading = false; }
}

function renderMessages() {
  const peers = [...snapshot.peers];
  for (const trusted of snapshot.trusted) { if (!peers.some((p) => p.id === trusted.id)) peers.push({ ...trusted, ready:false }); }
  if (!peers.some((p) => p.id === chosenPeer)) chosenPeer = peers[0]?.id || '';
  setHtml('chat-peer', peers.length ? peers.map((p) => `<option value="${escapeHtml(p.id)}">${escapeHtml(p.name)}${p.ready ? ' · 接続中' : ' · 未接続'}</option>`).join('') : '<option value="">先に端末をペアリングしてください</option>');
  $('chat-peer').value = chosenPeer;
  const available = snapshot.peers.some((p) => p.id === chosenPeer && p.ready);
  $('send-message').disabled = !available || busy.has('send-message');
  const messages = snapshot.messages.filter((m) => m.peer_id === chosenPeer);
  const signature = chosenPeer + messages.map((m) => m.id).join(',');
  if (signature !== messageSignature) {
    messageSignature = signature;
    setHtml('messages', messages.length ? messages.map((m) => `<article class="message ${m.direction === 'out' ? 'out' : ''}"><div class="message-body">${escapeHtml(m.text)}</div><div class="message-footer"><span>${m.channel === 'mesh.clipboard' ? 'クリップボード · ' : ''}${new Date(m.timestamp).toLocaleString('ja-JP',{month:'numeric',day:'numeric',hour:'2-digit',minute:'2-digit'})}</span>${button('コピー','copy-message',m.id,'copy-message')}</div></article>`).join('') : empty('メッセージはありません','接続した端末にテキストを送信できます。'));
    $('messages').scrollTop = $('messages').scrollHeight;
  }
}

function renderSettings() {
  if (document.activeElement !== $('device-name')) $('device-name').value = snapshot.device.name;
  setHtml('addresses', snapshot.device.addresses.length ? snapshot.device.addresses.map(escapeHtml).join('<br>') : 'LANに接続してください');
  $('fingerprint').textContent = snapshot.device.id;
  $('receive-dir').textContent = snapshot.receive_dir;
  $('storage-note').textContent = android ? '受信ファイルはアプリ内に保存されます。転送画面の「端末に保存…」から任意のフォルダーへ書き出してください。アプリを削除する前に必要なファイルを書き出してください。' : '受信したファイルはこのフォルダーへ保存します。既存のファイルは上書きしません。';
  setHtml('trusted', snapshot.trusted.length ? snapshot.trusted.map((p) => `<div class="trusted-peer"><div>${escapeHtml(p.name)}<small>${escapeHtml(p.id.slice(0,24))}…</small></div>${button('信頼を解除','forget',p.id,'danger-button')}</div>`).join('') : '<p class="muted small">まだペアリングした端末はありません。</p>');
}

let refreshing = false;
async function refresh() {
  if (refreshing || (!android && !desktop)) return;
  refreshing = true;
  try {
    snapshot = await command({ op:'snapshot' });
    $('native-error').hidden = true;
    render();
  } catch (error) {
    $('native-error').textContent = String(error.message || error);
    $('native-error').hidden = false;
    $('connection').lastElementChild.textContent = '接続を確認';
  } finally { refreshing = false; }
}

function confirmAction(title, text, work) {
  $('confirm-title').textContent = title;
  $('confirm-text').textContent = text;
  $('confirm-action').onclick = () => { $('confirm-dialog').close(); action('confirmation',work); };
  $('confirm-dialog').showModal();
}

document.addEventListener('click', (event) => {
  const target = event.target.closest('button');
  if (!target) return;
  if (target.dataset.tab) return tab(target.dataset.tab);
  if (target.dataset.close) return $(target.dataset.close).close();
  const id = target.dataset.id;
  const op = target.dataset.action;
  if (!op || !snapshot) return;
  const peer = snapshot.peers.find((p) => p.id === id);
  const transfer = snapshot.transfers.find((t) => t.id === id);
  if (op === 'chat') { chosenPeer = id; return tab('history'); }
  if (op === 'forget') return confirmAction('端末の信頼を解除', '接続を切断し、次回は再びコードの確認を求めます。', async () => { await command({op:'forget',peer_id:id}); toast('信頼を解除しました'); });
  action(`${op}-${id}`, async () => {
    target.disabled = true;
    try {
      switch (op) {
        case 'export-received': { const file = receivedFiles[Number(id)]; if (file) await exportReceived(file); break; }
        case 'connect': await command({op:'connect',peer_id:id,address:peer.address}); break;
        case 'pair': await command({op:'confirm',peer_id:id,code:peer.code}); toast('確認しました。相手側でも確認してください。'); break;
        case 'disconnect': await command({op:'disconnect',peer_id:id}); break;
        case 'file': { const result = await command({op:'pick_file',peer_id:id}); if (result) { tab('transfers'); toast('ファイルを準備しています'); } break; }
        case 'accept': await command({op:'accept_file',id,accept:true}); break;
        case 'reject': await command({op:'accept_file',id,accept:false}); break;
        case 'cancel': await command({op:'cancel_transfer',id}); break;
        case 'retry': await command({op:'send_file',peer_id:transfer.peer_id,path:transfer.path}); break;
        case 'export':
          await exportReceived(transfer);
          break;
        case 'copy-message': { const message = snapshot.messages.find((m) => m.id === id); if (message) { await command({op:'write_clipboard',text:message.text}); toast('コピーしました'); } break; }
      }
    } finally { target.disabled = false; }
  });
});

$('manual-open').onclick = () => { $('manual-dialog').showModal(); $('manual-address').focus(); };
$('received-refresh').onclick = () => refreshReceived(true);
$('received-prev').onclick = () => { receivedOffset = Math.max(0,receivedOffset - 50); refreshReceived(true); };
$('received-next').onclick = () => { receivedOffset += 50; refreshReceived(true); };
$('manual-form').onsubmit = (event) => {
  event.preventDefault();
  action('manual-connect', async () => {
    let address = $('manual-address').value.trim();
    if (!address.includes(':')) address += ':53319';
    await command({op:'connect',address}); $('manual-dialog').close(); toast('相手とペアリングコードを確認してください');
  });
};
$('chat-peer').onchange = () => { chosenPeer = $('chat-peer').value; renderMessages(); };
let clipboardDraft = false;
$('paste-clipboard').onclick = () => action('clipboard',async () => {
  const text = await command({op:'read_clipboard'});
  if (!text) return toast('クリップボードにテキストがありません');
  $('message').value = text;
  clipboardDraft = true;
  $('message').focus();
});
$('message-form').onsubmit = (event) => {
  event.preventDefault();
  action('send-message',async () => {
    const text = $('message').value;
    if (!text.trim()) return;
    if (new TextEncoder().encode(text).length > 16384) throw new Error('メッセージが長すぎます。16 KiB以内にしてください。');
    $('send-message').disabled = true;
    try { await command({op:'send_text',peer_id:chosenPeer,text,channel:clipboardDraft ? 'mesh.clipboard' : 'mesh.text'}); if ($('message').value === text) $('message').value = ''; clipboardDraft = false; }
    finally { $('send-message').disabled = false; }
  });
};
$('message').onkeydown = (event) => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.isComposing) { event.preventDefault(); $('message-form').requestSubmit(); } };
$('name-form').onsubmit = (event) => { event.preventDefault(); action('name',async () => { await command({op:'set_name',name:$('device-name').value}); toast('端末名を保存しました'); }); };
$('clear-history').onclick = () => confirmAction('メッセージ履歴を削除', 'この端末の履歴を削除します。相手の端末の履歴には影響しません。',async () => { await command({op:'clear_history'}); messageSignature = ''; toast('履歴を削除しました'); });

if (!android && !desktop) {
  $('native-error').innerHTML = 'この画面はLoCALアプリのUIです。<p>LANでの送受信にはAndroid APKまたはデスクトップアプリを起動してください。</p>';
  $('native-error').hidden = false;
  $('connection').lastElementChild.textContent = 'アプリ未接続';
  setHtml('peers', empty('LoCALアプリでつなごう','同じネットワークの端末を、自動で見つけます。'));
} else {
  refresh();
  setInterval(() => { if (!document.hidden) refresh(); }, 1000);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
}
