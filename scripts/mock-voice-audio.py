#!/usr/bin/env python3
"""A speech-to-text and text-to-speech upstream for a dev lmgw, with no model and no GPU: what
Kai's live check against a real lmgw (scripts/live-check-voice-lmgw.sh) points the dev instance's
ASR and TTS aliases at. It speaks the shapes of lmgw's own realtime test fakes
(crates/lmgw-core/tests/it/support/realtime_audio.rs, realtime_tts.rs), which live in its Rust
suite only; lmgw's standalone scripts/mock-openai.py answers `/audio/speech` with silence and has
no transcription route.

  POST /v1/audio/transcriptions   {"text": <ASR_TEXT>}, whatever was sent
  POST /v1/audio/speech           the WAV of <clip> (24 kHz mono PCM16), whatever the input
  GET  /v1/audio/voices           {"voices": ["alba"]}
  GET  /v1/models                 the two models

Each request is logged as one JSON line to stdout (path, time, the input's words for speech, the
upload's size for a transcription), so the check can say what lmgw asked for.

Environment: ASR_TEXT  what every transcription answers (default: a German question).

Usage: mock-voice-audio.py <port> <clip.wav>
"""
import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
with open(sys.argv[2], "rb") as f:
    CLIP = f.read()
ASR_TEXT = os.environ.get(
    "ASR_TEXT", "Wie wird das Wetter morgen in Berlin? Und was ziehe ich am besten an?"
)


def log(**kw):
    kw["t"] = round(time.time(), 3)
    print(json.dumps(kw, ensure_ascii=False), flush=True)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _send(self, code, ctype, body):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _json(self, code, obj):
        self._send(code, "application/json", json.dumps(obj).encode())

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        log(method="GET", path=path)
        if path.endswith("/audio/voices"):
            self._json(200, {"voices": ["alba"]})
        elif path.endswith("/models"):
            self._json(200, {"object": "list", "data": [
                {"id": "mock-asr", "object": "model"}, {"id": "mock-tts", "object": "model"}]})
        else:
            self._json(404, {"error": {"message": f"no route {path}"}})

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if path.endswith("/audio/transcriptions"):
            log(method="POST", path=path, bytes=len(raw))
            self._json(200, {"text": ASR_TEXT})
        elif path.endswith("/audio/speech"):
            try:
                body = json.loads(raw or b"{}")
            except ValueError:
                body = {}
            log(method="POST", path=path, input=body.get("input"), voice=body.get("voice"))
            self._send(200, "audio/wav", CLIP)
        else:
            log(method="POST", path=path, bytes=len(raw))
            self._json(404, {"error": {"message": f"no route {path}"}})


log(event="listening", port=PORT, clip_bytes=len(CLIP))
ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
