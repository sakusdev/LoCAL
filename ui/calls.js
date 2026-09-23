'use strict';

// Audio is WebRTC peer-to-peer. LoCAL's paired QUIC session only carries the
// authenticated offer/answer/ICE signals, so no public signaling service is needed.
(() => {
  if ((!android && !desktop) || typeof RTCPeerConnection !== 'function' ||
      !navigator.mediaDevices?.getUserMedia) return;

  let cursor = 0;
  let polling = false;
  let current = null;
  let incoming = null;
  let signalChain = Promise.resolve();
  const queuedCandidates = new Map();

  function compatiblePeers() {
    return (snapshot?.peers || []).filter((peer) => peer.ready && peer.capabilities?.includes('audio'));
  }

  function callPeerName(peerId) {
    return compatiblePeers().find((peer) => peer.id === peerId)?.name || peerName(peerId);
  }

  function updateUi() {
    $('calls-nav').hidden = false;
    const peers = compatiblePeers();
    if (current && !peers.some((peer) => peer.id === current.peerId)) {
      finishCall(false);
      toast('相手との接続が切れたため通話を終了しました',true);
      return;
    }
    const selected = $('call-peer').value;
    setHtml('call-peer', peers.length
      ? peers.map((peer) => `<option value="${escapeHtml(peer.id)}">${escapeHtml(peer.name)}</option>`).join('')
      : '<option value="">通話できる接続済み端末はありません</option>');
    if (peers.some((peer) => peer.id === selected)) $('call-peer').value = selected;
    $('call-start').disabled = Boolean(current) || !peers.length;
    $('call-start-panel').hidden = Boolean(current);
    $('call-active').hidden = !current;
    if (current) {
      $('call-peer-name').textContent = callPeerName(current.peerId);
      const labels = {ringing:'呼び出しています…', connecting:'接続しています…', active:'通話中', disconnected:'接続を回復しています…'};
      $('call-state-text').textContent = labels[current.state] || current.state;
      $('call-mute').textContent = current.muted ? 'ミュートを解除' : 'ミュート';
      $('call-duration').textContent = current.startedAt ? duration(Date.now() - current.startedAt) : '00:00';
    }
    $('call-offers').hidden = !incoming;
    if (incoming) {
      setHtml('call-offers', `<div><b>${escapeHtml(callPeerName(incoming.peer_id))}から音声通話です</b><p>応答するまでマイクは使用しません。</p><div class="actions"><button class="primary" data-call-action="accept">応答</button><button class="secondary" data-call-action="reject">拒否</button></div></div>`);
    } else setHtml('call-offers','');
  }

  function duration(milliseconds) {
    const seconds = Math.max(0,Math.floor(milliseconds / 1000));
    return `${String(Math.floor(seconds / 60)).padStart(2,'0')}:${String(seconds % 60).padStart(2,'0')}`;
  }

  function sendSignal(peerId, callId, kind, value = '') {
    const data = value === '' ? '' : JSON.stringify(value);
    signalChain = signalChain.catch(() => {}).then(() => command({op:'send_audio_signal',peer_id:peerId,call_id:callId,kind,data}));
    return signalChain;
  }

  async function microphone() {
    return navigator.mediaDevices.getUserMedia({video:false,audio:{echoCancellation:true,noiseSuppression:true,autoGainControl:true,channelCount:1}});
  }

  async function makeConnection(callId, peerId, state) {
    const stream = await microphone();
    const pc = new RTCPeerConnection({iceServers:[]});
    const call = {id:callId,peerId,state,pc,stream,muted:false,startedAt:0,candidates:queuedCandidates.get(callId) || []};
    queuedCandidates.delete(callId);
    current = call;
    for (const track of stream.getTracks()) pc.addTrack(track,stream);
    pc.onicecandidate = (event) => {
      if (event.candidate && current === call) {
        sendSignal(peerId,callId,'candidate',event.candidate.toJSON()).catch((error) => {
          toast(`通話の接続情報を送れません: ${error.message}`,true);
        });
      }
    };
    pc.ontrack = (event) => {
      if (current !== call) return;
      $('call-audio').srcObject = event.streams[0] || new MediaStream([event.track]);
      $('call-audio').play().catch(() => toast('音声を再生できません。端末の音量と再生許可を確認してください。',true));
    };
    pc.onconnectionstatechange = () => {
      if (current !== call) return;
      if (pc.connectionState === 'connected') {
        call.state = 'active';
        call.startedAt ||= Date.now();
      } else if (pc.connectionState === 'disconnected') call.state = 'disconnected';
      else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
        finishCall(false);
        toast('音声通話が終了しました',true);
      }
      updateUi();
    };
    updateUi();
    return call;
  }

  async function applyCandidates(call) {
    if (!call.pc.remoteDescription) return;
    const candidates = call.candidates.splice(0);
    for (const candidate of candidates) await call.pc.addIceCandidate(candidate);
  }

  async function startCall(peerId) {
    if (current) throw new Error('すでに通話中です');
    const callId = crypto.randomUUID();
    const call = await makeConnection(callId,peerId,'ringing');
    try {
      const offer = await call.pc.createOffer({offerToReceiveAudio:true});
      await call.pc.setLocalDescription(offer);
      await sendSignal(peerId,callId,'offer',call.pc.localDescription.toJSON());
      tab('calls');
      setTimeout(() => {
        if (current === call && call.state === 'ringing') {
          sendSignal(peerId,callId,'end').catch(() => {});
          finishCall(false);
          toast('相手が応答しませんでした',true);
        }
      },45000);
    } catch (error) {
      finishCall(false);
      throw error;
    }
  }

  async function acceptIncoming() {
    const offer = incoming;
    if (!offer || current) return;
    incoming = null;
    updateUi();
    try {
      const call = await makeConnection(offer.call_id,offer.peer_id,'connecting');
      await call.pc.setRemoteDescription(JSON.parse(offer.data));
      await applyCandidates(call);
      const answer = await call.pc.createAnswer();
      await call.pc.setLocalDescription(answer);
      await sendSignal(call.peerId,call.id,'answer',call.pc.localDescription.toJSON());
      tab('calls');
    } catch (error) {
      await sendSignal(offer.peer_id,offer.call_id,'reject').catch(() => {});
      finishCall(false);
      throw error;
    }
  }

  async function rejectIncoming() {
    const offer = incoming;
    incoming = null;
    updateUi();
    if (offer) await sendSignal(offer.peer_id,offer.call_id,'reject');
  }

  function finishCall(sendEnd = true) {
    const call = current;
    current = null;
    if (!call) return;
    if (sendEnd) sendSignal(call.peerId,call.id,'end').catch(() => {});
    call.pc.onconnectionstatechange = null;
    call.pc.onicecandidate = null;
    call.pc.ontrack = null;
    call.pc.close();
    for (const track of call.stream.getTracks()) track.stop();
    $('call-audio').pause();
    $('call-audio').srcObject = null;
    updateUi();
  }

  async function handleSignal(event) {
    if (!snapshot?.peers.some((peer) => peer.id === event.peer_id && peer.ready)) return;
    if (event.kind === 'offer') {
      if (current || incoming) await sendSignal(event.peer_id,event.call_id,'reject');
      else { incoming = event; updateUi(); }
      return;
    }
    if (!current || current.id !== event.call_id || current.peerId !== event.peer_id) {
      if (event.kind === 'candidate') {
        const list = queuedCandidates.get(event.call_id) || [];
        if (list.length < 64) list.push(JSON.parse(event.data));
        queuedCandidates.set(event.call_id,list);
      }
      return;
    }
    if (event.kind === 'answer') {
      await current.pc.setRemoteDescription(JSON.parse(event.data));
      current.state = 'connecting';
      await applyCandidates(current);
    } else if (event.kind === 'candidate') {
      const candidate = JSON.parse(event.data);
      if (current.pc.remoteDescription) await current.pc.addIceCandidate(candidate);
      else current.candidates.push(candidate);
    } else if (event.kind === 'reject') {
      finishCall(false);
      toast('相手が通話を拒否しました');
    } else if (event.kind === 'end') {
      finishCall(false);
      toast('通話が終了しました');
    }
    updateUi();
  }

  async function pollSignals() {
    if (polling || document.hidden || !snapshot) return;
    polling = true;
    try {
      const result = await command({op:'audio_signals',after:cursor});
      for (const event of result.events) {
        await handleSignal(event);
        cursor = Math.max(cursor,event.sequence);
      }
      cursor = Math.max(cursor,result.latest || 0);
    } catch (error) {
      if (current) toast(`通話の状態を取得できません: ${error.message}`,true);
    } finally { polling = false; }
  }

  document.addEventListener('click',(event) => {
    const target = event.target.closest('button');
    if (!target) return;
    if (target.dataset.action === 'call') {
      event.preventDefault();
      event.stopImmediatePropagation();
      $('call-peer').value = target.dataset.id;
      tab('calls');
      startCall(target.dataset.id).catch((error) => toast(`通話を開始できません: ${error.message}`,true));
    } else if (target.dataset.callAction === 'accept') {
      event.preventDefault();
      acceptIncoming().catch((error) => toast(`通話に応答できません: ${error.message}`,true));
    } else if (target.dataset.callAction === 'reject') {
      event.preventDefault();
      rejectIncoming().catch((error) => toast(`着信を拒否できません: ${error.message}`,true));
    }
  },true);

  $('call-start').onclick = () => startCall($('call-peer').value).catch((error) => toast(`通話を開始できません: ${error.message}`,true));
  $('call-end').onclick = () => finishCall(true);
  $('call-mute').onclick = () => {
    if (!current) return;
    current.muted = !current.muted;
    for (const track of current.stream.getAudioTracks()) track.enabled = !current.muted;
    updateUi();
  };
  window.addEventListener('beforeunload',() => finishCall(true));
  command({op:'enable_audio'}).then(() => {
    $('calls-nav').hidden = false;
    updateUi();
    setInterval(() => { updateUi(); pollSignals(); },500);
    refresh();
  }).catch((error) => toast(`音声通話を有効にできません: ${error.message}`,true));
})();
