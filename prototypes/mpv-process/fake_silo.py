# PROTOTYPE, throwaway. Serves sample.mp4 on 127.0.0.1:47613 only when the
# request carries "Authorization: Bearer proto-token". Logs each request.
import http.server, os, re

FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "sample.mp4")

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        auth = self.headers.get("Authorization")
        print(f"GET {self.path} auth={auth!r} range={self.headers.get('Range')!r}", flush=True)
        if auth != "Bearer proto-token":
            self.send_response(401); self.end_headers(); return
        size = os.path.getsize(FILE)
        start, end = 0, size - 1
        m = re.match(r"bytes=(\d+)-(\d*)", self.headers.get("Range", ""))
        if m:
            start = int(m[1]); end = int(m[2]) if m[2] else end
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        else:
            self.send_response(200)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Type", "video/mp4")
        self.send_header("Content-Length", str(end - start + 1))
        self.end_headers()
        with open(FILE, "rb") as f:
            f.seek(start)
            left = end - start + 1
            try:
                while left > 0:
                    chunk = f.read(min(65536, left))
                    self.wfile.write(chunk); left -= len(chunk)
            except (BrokenPipeError, ConnectionResetError):
                pass

    def log_message(self, *a): pass

http.server.ThreadingHTTPServer(("127.0.0.1", 47613), H).serve_forever()
