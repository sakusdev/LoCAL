'use strict';

(() => {
  const windowsSource = desktop && navigator.userAgent.includes('Windows');
  if (!windowsSource && !android) return;

  let supported = false;
  let sources = [];
  let viewerId = '';
  let decoder = null;
  let loadingSources = false;
  let androidPolling = false;
  const config = {codec:'avc1.42E01F',codedWidth:1280,codedHeight:720,avc:{format:'annexb'}};

  function closeViewer() {
    if (decoder && decoder.state !== 'closed') decoder.close();
    decoder = null;
    viewerId = '';
    $('screen-viewer').hidden = true;
  }

  function decodeFrame(payload) {
    if (!decoder || decoder.state !== 'configured') return;
    try {
      if (!payload.keyframe && decoder.decodeQueueSize > 4) return;
      const data = Uint8Array.from(atob(payload.data),(char) => char.charCodeAt(0));
      decoder.decode(new EncodedVideoChunk({type:payload.keyframe ? 'key' : 'delta',timestamp:payload.timestamp_us,data}));
    } catch (error) { toast('画面データを表示できません: ' + error.message,true); }
  }

  async function pollAndroidFrames(id) {
    if (!android || androidPolling) return;
    androidPolling = true;
    try {
      while (viewerId === id && decoder?.state === 'configured') {
        const result = await command({op:'poll_screen_frame',id});
        if (viewerId !== id || result.ended) break;
        if (result.frame) decodeFrame(result.frame);
      }
    } catch (error) {
      if (viewerId === id) toast('画面の受信が終了しました: ' + error.message,true);
    } finally {
      androidPolling = false;
      if (viewerId === id) closeViewer();
    }
  }

  function renderOffers() {
    const offers = (snapshot?.screen_sessions || []).filter((s) => s.direction === 'in' && s.status === 'offered');
    $('screen-offers').hidden = !offers.length;
    setHtml('screen-offers', offers.map((s) =>
      '<div><b>' + escapeHtml(peerName(s.peer_id)) + 'が画面共有を申し込んでいます</b>' +
      '<p>閲覧のみ · ' + s.offer.width + '×' + s.offer.height + ' · 音声と操作なし</p><div class="actions">' +
      button('画面を見る','screen-accept',s.id,'primary') + button('辞退','screen-reject',s.id) + '</div></div>'
    ).join(''));
  }

  function renderScreen() {
    if (!snapshot || activeTab !== 'screen') return;
    const peers = snapshot.peers.filter((p) => p.ready && p.screen?.decode?.codecs?.includes('h264'));
    const previous = $('screen-peer').value;
    setHtml('screen-peer', peers.map((p) => '<option value="' + escapeHtml(p.id) + '">' + escapeHtml(p.name) + '</option>').join('') ||
      '<option value="">画面表示に対応する接続済み端末はありません</option>');
    if (peers.some((p) => p.id === previous)) $('screen-peer').value = previous;
    $('screen-share').disabled = !peers.length || !sources.length;
    const sessions = snapshot.screen_sessions || [];
    setHtml('screen-sessions', sessions.length ? sessions.map((s) => {
      const actions = s.status === 'active'
        ? (s.direction === 'in' && viewerId !== s.id ? button('表示する','screen-watch',s.id,'primary') : '') +
          button('終了','screen-end',s.id,'danger-button') : '';
      return '<article class="transfer"><h3>' + escapeHtml(peerName(s.peer_id)) + ' · ' +
        (s.direction === 'in' ? '受信' : '共有') + '</h3><p>' + escapeHtml(s.status) + ' · ' +
        s.offer.width + '×' + s.offer.height + '</p>' +
        (s.error ? '<p class="error-text">' + escapeHtml(s.error) + '</p>' : '') +
        '<div class="actions">' + actions + '</div></article>';
    }).join('') : empty('画面共有はありません','ペアリング済みのWindows端末と画面を共有できます。'));
    if (viewerId && !sessions.some((s) => s.id === viewerId && s.status === 'active')) closeViewer();
  }

  async function refreshSources() {
    if (!supported || loadingSources) return;
    if (!windowsSource) {
      sources = [];
      $('screen-share-panel').hidden = true;
      return;
    }
    loadingSources = true;
    try {
      sources = (await command({op:'screen_sources'})).sources;
      setHtml('screen-source', sources.map((s) =>
        '<option value="' + escapeHtml(s.id) + '">' + escapeHtml(s.name) + ' · ' + s.width + '×' + s.height + '</option>'
      ).join('') || '<option value="">このPCでは画面を共有できません</option>');
    } catch {
      sources = [];
      setHtml('screen-source','<option value="">このPCでは画面を共有できません</option>');
    } finally {
      loadingSources = false;
      renderScreen();
    }
  }

  async function watch(id) {
    if (viewerId === id) { tab('screen'); return; }
    const session = (snapshot?.screen_sessions || []).find((s) => s.id === id && s.direction === 'in' && s.status === 'active');
    if (!session) throw new Error('画面共有はまだ開始されていません');
    closeViewer();
    const canvas = $('screen-canvas');
    canvas.width = session.offer.width;
    canvas.height = session.offer.height;
    const context = canvas.getContext('2d');
    decoder = new VideoDecoder({
      output(frame) { try { context.drawImage(frame,0,0,canvas.width,canvas.height); } finally { frame.close(); } },
      error(error) { toast('画面の復号に失敗しました: ' + error.message,true); },
    });
    decoder.configure({...config,codedWidth:canvas.width,codedHeight:canvas.height});
    viewerId = id;
    try { await command({op:'watch_screen',id}); }
    catch (error) { closeViewer(); throw error; }
    $('screen-viewer').hidden = false;
    tab('screen');
    pollAndroidFrames(id);
  }

  document.addEventListener('click',(event) => {
    const target = event.target.closest('button');
    if (!target) return;
    if (target.dataset.tab === 'screen') { refreshSources(); return; }
    const op = target.dataset.action;
    if (!op?.startsWith('screen-')) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    const id = target.dataset.id;
    action(op + id,async () => {
      target.disabled = true;
      try {
        if (op === 'screen-accept') { await command({op:'accept_screen',id,accept:true}); tab('screen'); }
        else if (op === 'screen-reject') await command({op:'accept_screen',id,accept:false});
        else if (op === 'screen-watch') await watch(id);
        else if (op === 'screen-end') {
          if (id === viewerId) closeViewer();
          await command({op:'stop_screen',id});
        }
      } finally { target.disabled = false; }
    });
  },true);

  $('screen-refresh-sources').onclick = refreshSources;
  $('screen-share').onclick = () => action('screen-share',async () => {
    $('screen-share').disabled = true;
    try {
      const result = await command({op:'share_screen',peer_id:$('screen-peer').value,
        source_id:$('screen-source').value,max_width:1280,max_height:720,max_fps:15});
      toast(result.accepted ? '画面を共有しています' : '相手が画面共有を辞退しました');
    } finally { $('screen-share').disabled = false; }
  });
  $('screen-stop').onclick = () => action('screen-stop',async () => {
    const id = viewerId;
    closeViewer();
    if (id) await command({op:'stop_screen',id});
  });

  async function boot() {
    if (typeof VideoDecoder !== 'function' || typeof EncodedVideoChunk !== 'function' ||
        !(await VideoDecoder.isConfigSupported(config)).supported) return;
    if (desktop) {
      await window.__TAURI__.event.listen('local-screen-frame',({payload}) => {
        if (payload.id === viewerId) decodeFrame(payload);
      });
      await window.__TAURI__.event.listen('local-screen-ended',({payload}) => {
        if (payload.id === viewerId) closeViewer();
      });
    }
    await command({op:'enable_screen_view'});
    supported = true;
    $('screen-nav').hidden = false;
    await refreshSources();
    await refresh();
  }

  boot().catch((error) => toast('画面共有の準備に失敗しました: ' + error.message,true));
  setInterval(() => {
    if (!supported || !snapshot) return;
    renderOffers();
    renderScreen();
  },700);
})();
