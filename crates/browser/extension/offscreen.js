// The extension's offscreen page: records a tab as video while the user
// records a bug report (Chrome lets only a page, not the worker, record).
// The video stays here until the recording stops; then it goes to the
// worker with a few still frames, and nothing is kept.

/** Most still frames kept (one every few seconds); a few go with the report. */
const MAX_FRAMES = 90;
const FRAME_EVERY = 2000;

let recording = null;

async function start(streamId) {
  if (recording) throw new Error('already recording');
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: false,
    video: {
      mandatory: {
        chromeMediaSource: 'tab',
        chromeMediaSourceId: streamId,
        maxWidth: 1600,
        maxHeight: 1000,
        maxFrameRate: 15,
      },
    },
  });
  const type = ['video/webm;codecs=vp9', 'video/webm;codecs=vp8', 'video/webm']
    .find((candidate) => MediaRecorder.isTypeSupported(candidate));
  const recorder = new MediaRecorder(stream, { mimeType: type, videoBitsPerSecond: 800000 });
  const chunks = [];
  recorder.ondataavailable = (event) => {
    if (event.data.size > 0) chunks.push(event.data);
  };
  recorder.start(1000);

  // Still frames, from the same stream.
  const video = document.createElement('video');
  video.muted = true;
  video.srcObject = stream;
  await video.play().catch(() => {});
  const frames = [];
  const grab = async () => {
    if (!video.videoWidth) return;
    const scale = Math.min(1, 1280 / video.videoWidth);
    const canvas = new OffscreenCanvas(Math.round(video.videoWidth * scale), Math.round(video.videoHeight * scale));
    canvas.getContext('2d').drawImage(video, 0, 0, canvas.width, canvas.height);
    frames.push(await canvas.convertToBlob({ type: 'image/png' }));
    if (frames.length > MAX_FRAMES) frames.splice(1, 1);
  };
  const timer = setInterval(() => void grab(), FRAME_EVERY);
  setTimeout(() => void grab(), 300);
  recording = { stream, recorder, chunks, frames, timer, video, grab };
}

async function stop(keep) {
  if (!recording) return { video: null, frames: [] };
  const { stream, recorder, chunks, frames, timer, video, grab } = recording;
  recording = null;
  clearInterval(timer);
  if (keep) await grab();
  const ended = new Promise((resolve) => {
    recorder.onstop = resolve;
  });
  if (recorder.state !== 'inactive') recorder.stop();
  await ended;
  for (const track of stream.getTracks()) track.stop();
  video.srcObject = null;
  if (!keep) return { video: null, frames: [] };
  const picked = pick(frames, 4);
  return {
    video: await base64(new Blob(chunks, { type: 'video/webm' })),
    frames: await Promise.all(picked.map(base64)),
  };
}

/** `count` frames spread over the recording, always the last one. */
function pick(frames, count) {
  if (frames.length <= count) return frames;
  const out = [];
  for (let i = 0; i < count; i += 1) {
    out.push(frames[Math.round((i * (frames.length - 1)) / (count - 1))]);
  }
  return out;
}

async function base64(blob) {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let text = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(text);
}

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (message?.target !== 'offscreen' || sender.id !== chrome.runtime.id) return false;
  const work = message.type === 'start' ? start(message.streamId).then(() => ({})) : stop(!!message.keep);
  work.then(
    (result) => respond({ ok: true, result }),
    (error) => respond({ ok: false, error: String(error?.message ?? error) }),
  );
  return true;
});
