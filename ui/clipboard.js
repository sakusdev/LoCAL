'use strict';

(() => {
  const STORAGE_ENABLED = 'local.clipboard.enabled';
  const STORAGE_PEERS = 'local.clipboard.peers';
  let enabled = localStorage.getItem(STORAGE_ENABLED) === 'true';
  let storedPeers = [];
  try { storedPeers = JSON.parse(localStorage.getItem(STORAGE_PEERS) || '[]'); } catch {}
  let selected = new Set(Array.isArray(storedPeers) ? storedPeers : []);
  let baselineReady = false;
  let lastClipboard = null;
  let sending = false;
  let seen = new Set();
  let recent = [];
  let clipboardRevision = null;

  function save() {
    localStorage.setItem(STORAGE_ENABLED, String(enabled));
    localStorage.setItem(STORAGE_PEERS, JSON.stringify([...selected]));
  }

  function eligiblePeers() {
    return (snapshot?.peers || []).filter((peer) => peer.ready && peer.capabilities?.includes('clipboard'));
  }

  function shortText(text) {
    const normalized = String(text || '').replace(/\s+/g, ' ').trim();
    return normalized.length > 120 ? normalized.slice(0, 117) + '…' : normalized;
  }

  function remember(direction, peerId, text) {
    recent.unshift({direction,peerId,text,time:Date.now()});
    recent = recent.slice(0, 12);
  }

  function renderClipboard() {
    if (!snapshot) return;
    const peers = eligiblePeers();
    $('clipboard-enabled').checked = enabled;
    $('clipboard-state').textContent = enabled
      ? `${peers.filter((peer) => selected.has(peer.id)).length}台と同期中 · LoCALを開いている間のみ`
      : '停止中';
    setHtml('clipboard-peers', peers.length ? peers.map((peer) =>
      `<div class="clipboard-peer"><label><input type="checkbox" data-clipboard-peer="${escapeHtml(peer.id)}" ${selected.has(peer.id) ? 'checked' : ''}><span>${escapeHtml(peer.name)}</span></label><span class="muted small">接続中</span></div>`
    ).join('') : empty('同期できる端末がありません','先に相手をペアリングして接続してください。'));
    setHtml('clipboard-events', recent.length ? recent.map((event) =>
      `<article class="clipboard-event"><strong>${event.direction === 'out' ? '送信' : '受信'} · ${escapeHtml(peerName(event.peerId))}</strong><span class="muted small"> · ${new Date(event.time).toLocaleTimeString('ja-JP',{hour:'2-digit',minute:'2-digit'})}</span><p>${escapeHtml(shortText(event.text) || '空のテキスト')}</p></article>`
    ).join('') : empty('まだ同期はありません','同期を有効にして、テキストをコピーしてください。'));
    $('clipboard-send-now').disabled = !peers.some((peer) => selected.has(peer.id));
    $('clipboard-pull-latest').disabled = !snapshot.messages.some((message) => message.direction === 'in' && message.channel === 'mesh.clipboard' && selected.has(message.peer_id));
  }

  async function currentText() {
    const text = await command({op:'read_clipboard'});
    if (!text || new TextEncoder().encode(text).length > 16384) return null;
    return text;
  }

  async function changedText() {
    if (android) {
      const revision = await command({op:'clipboard_revision'});
      if (clipboardRevision === revision) return {changed:false,text:null};
      clipboardRevision = revision;
    }
    return {changed:true,text:await currentText()};
  }

  async function send(text, manual = false) {
    if (sending) return;
    const peers = eligiblePeers().filter((peer) => selected.has(peer.id));
    if (!peers.length) { if (manual) toast('同期する接続済み端末を選んでください'); return; }
    if (!text) { if (manual) toast('クリップボードに送れるテキストがありません'); return; }
    sending = true;
    try {
      const results = await Promise.allSettled(peers.map((peer) => command({op:'send_text',peer_id:peer.id,text,channel:'mesh.clipboard'})));
      results.forEach((result,index) => { if (result.status === 'fulfilled') remember('out',peers[index].id,text); });
      const failed = results.filter((result) => result.status === 'rejected');
      if (manual) toast(failed.length ? `${results.length - failed.length}台へ送信、${failed.length}台は失敗しました` : `${results.length}台へ送りました`);
      if (failed.length === results.length && failed[0]) throw failed[0].reason;
    } finally { sending = false; renderClipboard(); }
  }

  async function tick() {
    if (!snapshot || document.hidden) return;
    const incoming = snapshot.messages.filter((message) => message.direction === 'in' && message.channel === 'mesh.clipboard');
    if (!baselineReady) {
      incoming.forEach((message) => seen.add(message.id));
      try {
        if (android) clipboardRevision = await command({op:'clipboard_revision'});
        lastClipboard = await currentText();
      } catch {}
      baselineReady = true;
      return;
    }
    if (!enabled) return;
    for (const message of incoming.slice().reverse()) {
      if (seen.has(message.id)) continue;
      seen.add(message.id);
      if (!selected.has(message.peer_id)) continue;
      await command({op:'write_clipboard',text:message.text});
      lastClipboard = message.text;
      remember('in',message.peer_id,message.text);
      toast(`${peerName(message.peer_id)}からクリップボードを受信しました`);
    }
    const changed = await changedText();
    const text = changed.text;
    if (changed.changed && text !== null && lastClipboard !== null && text !== lastClipboard) {
      lastClipboard = text;
      await send(text);
    } else if (changed.changed && lastClipboard === null) lastClipboard = text;
    renderClipboard();
  }

  $('clipboard-enabled').onchange = async () => {
    enabled = $('clipboard-enabled').checked;
    save();
    baselineReady = false;
    if (enabled) {
      if (!selected.size) eligiblePeers().forEach((peer) => selected.add(peer.id));
      save();
      toast('クリップボード同期を開始しました');
    } else toast('クリップボード同期を停止しました');
    renderClipboard();
  };
  $('clipboard-peers').onchange = (event) => {
    const id = event.target.dataset.clipboardPeer;
    if (!id) return;
    event.target.checked ? selected.add(id) : selected.delete(id);
    save(); renderClipboard();
  };
  $('clipboard-select-all').onclick = () => { eligiblePeers().forEach((peer) => selected.add(peer.id)); save(); renderClipboard(); };
  $('clipboard-send-now').onclick = () => action('clipboard-send-now',async () => send(await currentText(),true));
  $('clipboard-pull-latest').onclick = () => action('clipboard-pull-latest',async () => {
    const message = snapshot.messages.find((item) => item.direction === 'in' && item.channel === 'mesh.clipboard' && selected.has(item.peer_id));
    if (!message) return toast('受信したクリップボードはありません');
    await command({op:'write_clipboard',text:message.text});
    lastClipboard = message.text;
    remember('in',message.peer_id,message.text);
    toast('最新の受信テキストをコピーしました');
  });
  document.addEventListener('click',(event) => { if (event.target.closest('[data-tab="clipboard"]')) setTimeout(renderClipboard); });
  setInterval(() => tick().catch((error) => { if (enabled) toast('クリップボード同期: ' + (error.message || error),true); }),1200);
  setInterval(renderClipboard,900);
})();
