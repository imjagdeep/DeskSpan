// The fake host page (see dev-host.mjs): an animated test picture as the
// "screen", one RTCPeerConnection per receiver, input logged.
// window.__inputs and window.__peers are read by test scripts.

const canvas = document.getElementById("c");
const g = canvas.getContext("2d");
const log = document.getElementById("log");
window.__inputs = [];
window.__peers = {};

let t = 0;
const dots = [];
function draw() {
  t++;
  g.fillStyle = "#1d3557";
  g.fillRect(0, 0, 1280, 800);
  g.strokeStyle = "#f1faee";
  g.lineWidth = 8;
  g.strokeRect(4, 4, 1272, 792);
  g.fillStyle = "#e63946";
  g.fillRect(100 + ((t * 6) % 1000), 360, 80, 80);
  g.fillStyle = "#f1faee";
  g.font = "bold 64px system-ui";
  g.fillText(`DeskSpan test picture ${new Date().toLocaleTimeString()}`, 60, 120);
  g.fillStyle = "#a8dadc";
  for (const d of dots) g.fillRect(d.x * 1280 - 6, d.y * 800 - 6, 12, 12);
}
// A timer, not requestAnimationFrame: that pauses in background tabs.
setInterval(draw, 33);
draw();
const stream = canvas.captureStream(30);

const ws = new WebSocket(`ws://${location.host}/host`);
const send = (m) => ws.send(JSON.stringify(m));
const say = (s) => {
  log.textContent = `${s}\n${log.textContent}`.slice(0, 5000);
};

ws.onmessage = async (ev) => {
  const m = JSON.parse(ev.data);
  if (m.t === "join") {
    const pc = new RTCPeerConnection({ iceServers: [] });
    window.__peers[m.session] = pc;
    for (const track of stream.getTracks()) pc.addTrack(track, stream);
    const dc = pc.createDataChannel("input");
    dc.onmessage = (e) => {
      const ev = JSON.parse(e.data);
      window.__inputs.push(ev);
      if (ev.k === "p" && ev.e !== "u") dots.push({ x: ev.x, y: ev.y });
      say(`#${m.session} ${e.data}`);
    };
    pc.onicecandidate = (e) => e.candidate && send({ t: "ice", session: m.session, c: e.candidate.toJSON() });
    pc.onconnectionstatechange = () => say(`#${m.session} ${pc.connectionState}`);
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    send({ t: "offer", session: m.session, sdp: pc.localDescription.sdp });
    say(`#${m.session} joined, offer sent`);
  } else if (m.t === "answer") {
    await window.__peers[m.session]?.setRemoteDescription({ type: "answer", sdp: m.sdp });
  } else if (m.t === "ice") {
    await window.__peers[m.session]?.addIceCandidate(m.c).catch((e) => say(`ice error ${e}`));
  } else if (m.t === "leave") {
    window.__peers[m.session]?.close();
    delete window.__peers[m.session];
    say(`#${m.session} left`);
  }
};
